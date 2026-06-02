//! Shared source-operation kernel for `survey` and `extract`
//! (REVIEW.md A6).
//!
//! `survey.rs` and `extract.rs` run the same two-phase agent/tool flow
//! around an adapter-declared brief: resolve the sandbox scratch path,
//! read the staged artifact, build the closed [`CacheFingerprint`], and
//! append the cache index row. The only axis of variation is the
//! [`SourceOperation`] (`survey` vs `extract`) and its `lead` input, so
//! these helpers are parameterised by it and preserve each operation's
//! wire-stable diagnostic codes via a match on the op.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use specify_error::{Error, Result};
use specify_workflow::adapter::cache::{
    self, CacheFingerprint, CacheIndexEntry, FingerprintSource, FingerprintToolVersion,
};
use specify_workflow::adapter::{CacheLayout, CacheMode, SourceOperation};
use specify_workflow::change::SourceBinding;

use crate::runtime::commands::source::prep;

/// The `$SCRATCH_DIR` host path the prep mounted for this operation.
///
/// `survey` / `extract` prep always mounts a scratch root (preflight
/// §1); a `None` is an unreachable prep-invariant violation surfaced as
/// a diagnostic rather than a panic (REVIEW.md A6).
///
/// # Errors
///
/// Returns `source-scratch-missing` when the prep mounted no scratch
/// root (an unreachable prep-invariant violation).
pub(super) fn scratch_path(prepared: &prep::SourcePrep, op: SourceOperation) -> Result<PathBuf> {
    prepared.layout.scratch.path.clone().ok_or_else(|| Error::Diag {
        code: "source-scratch-missing",
        detail: format!("{op} prep mounted no $SCRATCH_DIR host path"),
    })
}

/// Read the staged artifact (`lead-set.md` / `evidence.yaml`), mapping a
/// missing file to the operation's wire-stable "must write into
/// `$SCRATCH_DIR` before finalize" diagnostic.
///
/// # Errors
///
/// Returns the operation's `*-missing` diagnostic when the artifact is
/// absent, or [`Error::Io`] on any other read failure.
pub(super) fn read_artifact(path: &Path, op: SourceOperation) -> Result<String> {
    std::fs::read_to_string(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            let (code, what, writer) = match op {
                SourceOperation::Survey => {
                    ("survey-lead-set-missing", "lead-set.md", "the survey must write the lead set")
                }
                SourceOperation::Extract => (
                    "extract-evidence-missing",
                    "evidence.yaml",
                    "the extract must write the Evidence",
                ),
            };
            Error::Diag {
                code,
                detail: format!(
                    "no `{what}` at {}; {writer} into $SCRATCH_DIR before finalize",
                    path.display()
                ),
            }
        } else {
            Error::Io(err)
        }
    })
}

/// Build the closed [`CacheFingerprint`] (RFC-27) for a source
/// operation: source identity, `<name>@<version>`, the operation's
/// brief sha256, the declared tool versions, and the optional `lead`
/// input (`None` for `survey`, `Some(<lead>)` for `extract`).
///
/// # Errors
///
/// - the operation's `*-brief-missing` diagnostic when the manifest
///   declares no brief for it.
/// - the operation's `*-brief-read-failed` diagnostic on a brief read
///   error.
/// - propagates [`FingerprintSource::from_path`] failures.
pub(super) fn build_fingerprint(
    prepared: &prep::SourcePrep, source_path: Option<&Path>, binding: &SourceBinding,
    op: SourceOperation, lead: Option<String>,
) -> Result<CacheFingerprint> {
    let source = match source_path {
        Some(path) => FingerprintSource::from_path(path)?,
        None => {
            FingerprintSource::from_value(binding.value.as_deref().unwrap_or_default().as_bytes())
        }
    };
    let adapter = format!("{}@{}", prepared.manifest.name, prepared.manifest.version);

    let (missing_code, read_code, label) = match op {
        SourceOperation::Survey => ("survey-brief-missing", "survey-brief-read-failed", "survey"),
        SourceOperation::Extract => {
            ("extract-brief-missing", "extract-brief-read-failed", "extract")
        }
    };
    let brief_relative = prepared.manifest.briefs.get(&op).ok_or_else(|| Error::Diag {
        code: missing_code,
        detail: format!("source adapter `{}` declares no `{label}` brief", prepared.manifest.name),
    })?;
    let brief_path = prepared.adapter_dir.join(brief_relative);
    let brief_bytes = std::fs::read(&brief_path).map_err(|err| Error::Diag {
        code: read_code,
        detail: format!("failed to read {label} brief {}: {err}", brief_path.display()),
    })?;

    let tool_versions = prepared
        .manifest
        .tools
        .iter()
        .map(|tool| FingerprintToolVersion {
            name: tool.name.clone(),
            version: tool.version.clone(),
        })
        .collect();

    Ok(CacheFingerprint::new(
        source,
        adapter,
        cache::sha256_prefixed(&brief_bytes),
        tool_versions,
        lead,
    ))
}

/// Per-write cache-entry identity bundle, keeping
/// [`write_cache_entry`] under the argument-count budget.
pub(super) struct CacheEntry<'a> {
    pub layout: CacheLayout<'a>,
    pub cache_mode: Option<CacheMode>,
    /// Cache-index `slice` lane (`survey` for the slice-less survey op,
    /// the slice name for extract).
    pub slice_lane: &'a str,
    pub source: &'a str,
    pub adapter: &'a str,
    pub op: SourceOperation,
}

/// Write the cache artifact + `fingerprint.json` + index row for a
/// source operation. Under the forced opt-out the cache layer skips the
/// directory body and appends only the audit index row.
///
/// # Errors
///
/// Propagates [`cache::write`] failures.
pub(super) fn write_cache_entry(
    entry: &CacheEntry<'_>, fingerprint: &CacheFingerprint, artifact_bytes: &[u8],
) -> Result<()> {
    let index = CacheIndexEntry {
        timestamp: Timestamp::now(),
        fingerprint: fingerprint.digest(),
        slice: entry.slice_lane.to_string(),
        source: entry.source.to_string(),
        adapter: entry.adapter.to_string(),
        operation: entry.op,
    };
    cache::write(
        entry.layout,
        fingerprint,
        artifact_bytes,
        entry.op.artifact_name(),
        entry.cache_mode,
        &index,
    )
}
