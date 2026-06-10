//! extraction cache fingerprint contract cache lookup / write / index helpers.
//!
//! Cache directory layout (extraction cache fingerprint contract):
//!
//! ```text
//! .specify/cache/extractions/<adapter>/
//!     <fingerprint>/
//!         evidence.yaml      # or leads.md for survey
//!         fingerprint.json   # full input record for audit
//!     index.jsonl            # one row per cache write; append-only
//! ```
//!
//! The extraction cache is per-adapter only (not per-axis) — only
//! source adapters extract — and lives in its own root, disjoint from
//! the per-axis manifest cache at
//! `.specify/cache/manifests/{sources,targets}/<name>/` and from the
//! per-operation agent scratch lanes at the sibling
//! `.specify/cache/scratch/<adapter>/{survey,<slice>}/` root
//! ([`crate::adapter::scratch_dir`]), so everything under
//! `extractions/` is fingerprint-keyed cache content. See
//! [DECISIONS.md §"Cache layout"].
//!
//! Atomic writes mirror [DECISIONS.md §"Atomic writes"]: the cache
//! directory body uses [`bytes_write`] (tempfile + rename), while the
//! per-row index appends go through `O_APPEND` exactly like
//! `journal::append_batch`.
//!
//! [DECISIONS.md §"Atomic writes"]: ../../../../DECISIONS.md#atomic-writes
//! [DECISIONS.md §"Cache layout"]: ../../../../DECISIONS.md#cache-layout
//! [`bytes_write`]: specify_model::atomic::bytes_write

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;
use specify_model::atomic::bytes_write;

use crate::adapter::CacheMode;
use crate::adapter::cache::{CacheFingerprint, CacheIndexEntry, CacheMissReason, SourceOperation};
use crate::adapter::core::EXTRACTIONS_CACHE_DIR;

const INDEX_FILE_NAME: &str = "index.jsonl";
const FINGERPRINT_RECORD_NAME: &str = "fingerprint.json";

/// Filesystem coordinates for the extraction cache fingerprint contract cache scoped to one
/// source adapter.
///
/// Construct with [`CacheLayout::new`]; the type is path-only and
/// performs no I/O of its own.
#[derive(Debug, Clone, Copy)]
pub struct CacheLayout<'a> {
    project_dir: &'a Path,
    adapter: &'a str,
}

impl<'a> CacheLayout<'a> {
    /// Pair `project_dir` with `adapter` (the source-adapter name).
    #[must_use]
    pub const fn new(project_dir: &'a Path, adapter: &'a str) -> Self {
        Self { project_dir, adapter }
    }

    /// `.specify/cache/extractions/<adapter>/`.
    #[must_use]
    pub fn adapter_dir(&self) -> PathBuf {
        self.project_dir
            .join(".specify")
            .join("cache")
            .join(EXTRACTIONS_CACHE_DIR)
            .join(self.adapter)
    }

    /// `.specify/cache/extractions/<adapter>/<fingerprint-sha256>/`.
    #[must_use]
    pub fn fingerprint_dir(&self, digest: &str) -> PathBuf {
        self.adapter_dir().join(digest_dir_name(digest))
    }

    /// `.specify/cache/extractions/<adapter>/<fp>/fingerprint.json`.
    #[must_use]
    pub fn fingerprint_record_path(&self, digest: &str) -> PathBuf {
        self.fingerprint_dir(digest).join(FINGERPRINT_RECORD_NAME)
    }

    /// `.specify/cache/extractions/<adapter>/<fp>/<artifact-name>`.
    #[must_use]
    pub fn artifact_path(&self, digest: &str, artifact_name: &str) -> PathBuf {
        self.fingerprint_dir(digest).join(artifact_name)
    }

    /// `.specify/cache/extractions/<adapter>/index.jsonl`.
    #[must_use]
    pub fn index_path(&self) -> PathBuf {
        self.adapter_dir().join(INDEX_FILE_NAME)
    }
}

/// `fingerprint.json` payload — the closed five-input record persisted
/// alongside the cached artifact for audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FingerprintRecord {
    /// sha256 hex digest computed over [`Self::inputs`] at write time.
    pub digest: String,
    /// Closed input record per the extraction cache fingerprint contract.
    pub inputs: CacheFingerprint,
}

impl FingerprintRecord {
    /// Pair the inputs with their digest. Callers usually build the
    /// digest via [`CacheFingerprint::digest`] before constructing the
    /// record.
    #[must_use]
    pub fn new(inputs: CacheFingerprint) -> Self {
        let digest = inputs.digest();
        Self { digest, inputs }
    }
}

/// Outcome of [`lookup`] — either a hit on the cache directory or a
/// miss carrying the closed [`CacheMissReason`] discriminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupOutcome {
    /// Cache directory exists with a valid `fingerprint.json` whose
    /// digest matches the current inputs.
    Hit {
        /// Path to the `<fp>/` directory containing the cached
        /// artifact.
        cache_dir: PathBuf,
    },
    /// Cache directory does not exist, the adapter declared
    /// `cache: opt-out`, or the prior `fingerprint.json` could not be
    /// parsed.
    Miss {
        /// Which fingerprint input drifted (or `no-prior-entry` /
        /// `adapter-opt-out`).
        reason: CacheMissReason,
    },
}

/// Result of [`lookup`] — the computed digest, the cache directory the
/// hit would have lived in, and the closed [`LookupOutcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLookup {
    /// sha256 hex digest of the current inputs.
    pub digest: String,
    /// `.specify/cache/extractions/<adapter>/<fp>/` regardless of hit /
    /// miss. Operators see the path even on a miss so they know where
    /// the upcoming write will land.
    pub cache_dir: PathBuf,
    /// Hit / miss with reason.
    pub outcome: LookupOutcome,
}

/// Look up the cache for `fingerprint` and report hit vs. miss with a
/// reason.
///
/// Probe order:
///
/// 1. `cache_mode == Some(CacheMode::OptOut)` → miss with
///    [`CacheMissReason::AdapterOptOut`] (skip every filesystem read).
/// 2. `<adapter>/<digest>/fingerprint.json` exists → hit.
/// 3. Otherwise, scan `index.jsonl` for the most recent prior entry on
///    the same `(slice, source, operation)` lane, load that
///    entry's `fingerprint.json`, and diff field-by-field per
///    [`CacheFingerprint::diff_reason`].
/// 4. No prior entry (or unreadable / corrupt prior record) →
///    [`CacheMissReason::NoPriorEntry`].
///
/// # Errors
///
/// Propagates I/O failures reading the index log. A corrupt
/// `fingerprint.json` on a prior entry is **not** an error — the miss
/// is reported as [`CacheMissReason::NoPriorEntry`] and the operator
/// can rebuild the cache.
pub fn lookup(
    layout: CacheLayout<'_>, fingerprint: &CacheFingerprint, cache_mode: Option<CacheMode>,
    slice: &str, source: &str, operation: SourceOperation,
) -> Result<CacheLookup, Error> {
    let digest = fingerprint.digest();
    let cache_dir = layout.fingerprint_dir(&digest);

    if matches!(cache_mode, Some(CacheMode::OptOut)) {
        return Ok(CacheLookup {
            digest,
            cache_dir,
            outcome: LookupOutcome::Miss {
                reason: CacheMissReason::AdapterOptOut,
            },
        });
    }

    let record_path = layout.fingerprint_record_path(&digest);
    if record_path.is_file() {
        return Ok(CacheLookup {
            digest,
            cache_dir: cache_dir.clone(),
            outcome: LookupOutcome::Hit { cache_dir },
        });
    }

    let reason = miss_reason(layout, fingerprint, slice, source, operation)?;
    Ok(CacheLookup {
        digest,
        cache_dir,
        outcome: LookupOutcome::Miss { reason },
    })
}

fn miss_reason(
    layout: CacheLayout<'_>, fingerprint: &CacheFingerprint, slice: &str, source: &str,
    operation: SourceOperation,
) -> Result<CacheMissReason, Error> {
    let entries = read_index(layout)?;
    let Some(prior) = entries
        .into_iter()
        .rev()
        .find(|e| e.slice == slice && e.source == source && e.operation == operation)
    else {
        return Ok(CacheMissReason::NoPriorEntry);
    };

    let prior_record_path = layout.fingerprint_record_path(&prior.fingerprint);
    let Ok(raw) = std::fs::read_to_string(&prior_record_path) else {
        // Either the record is missing (operator cleared the cache
        // directory but kept the index) or the file is unreadable;
        // either way no diff is possible.
        return Ok(CacheMissReason::NoPriorEntry);
    };
    let Ok(record) = serde_json::from_str::<FingerprintRecord>(&raw) else {
        // Cache-corruption — extraction cache fingerprint contract leans on "warn and treat as
        // miss" rather than failing the whole operation.
        return Ok(CacheMissReason::NoPriorEntry);
    };
    Ok(CacheFingerprint::diff_reason(&record.inputs, fingerprint)
        .unwrap_or(CacheMissReason::NoPriorEntry))
}

/// Write a cache entry: artifact bytes, `fingerprint.json` record, and
/// an `index.jsonl` row.
///
/// When `cache_mode == Some(CacheMode::OptOut)` the function still
/// appends the index row (so the audit log carries the opt-out trail)
/// but **does not** create the `<digest>/` directory or its contents.
///
/// # Errors
///
/// Propagates I/O failures from the directory create, atomic
/// tempfile-rename of the artifact / record, and the index append.
pub fn write(
    layout: CacheLayout<'_>, fingerprint: &CacheFingerprint, artifact_bytes: &[u8],
    artifact_name: &str, cache_mode: Option<CacheMode>, entry: &CacheIndexEntry,
) -> Result<(), Error> {
    if !matches!(cache_mode, Some(CacheMode::OptOut)) {
        let digest = fingerprint.digest();
        let artifact_path = layout.artifact_path(&digest, artifact_name);
        bytes_write(&artifact_path, artifact_bytes)?;

        let record = FingerprintRecord::new(fingerprint.clone());
        let record_bytes = serde_json::to_vec_pretty(&record).map_err(|err| Error::Diag {
            code: "cache-fingerprint-record-serialise-failed",
            detail: format!("failed to serialise fingerprint.json: {err}"),
        })?;
        bytes_write(&layout.fingerprint_record_path(&digest), &record_bytes)?;
    }

    append_index(layout, entry)?;
    Ok(())
}

/// Append one [`CacheIndexEntry`] to
/// `.specify/cache/extractions/<adapter>/index.jsonl`.
///
/// Mirrors `journal::append_batch`: the directory is created on first write,
/// the row is emitted as a single JSON line followed by `\n`, and the
/// open uses `O_APPEND` so concurrent writers on a local filesystem
/// cannot interleave (writes under `PIPE_BUF` are atomic).
///
/// # Errors
///
/// Propagates I/O failures from the directory create, file open, or
/// row write, plus JSON serialisation failures as
/// `cache-index-entry-serialise-failed`.
pub fn append_index(layout: CacheLayout<'_>, entry: &CacheIndexEntry) -> Result<(), Error> {
    std::fs::create_dir_all(layout.adapter_dir())?;
    let path = layout.index_path();
    let line = serde_json::to_string(entry).map_err(|err| Error::Diag {
        code: "cache-index-entry-serialise-failed",
        detail: format!("failed to serialise cache index entry: {err}"),
    })?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{line}")?;
    file.sync_all()?;
    Ok(())
}

/// Read every row in `index.jsonl` in append order.
///
/// Returns `Ok(vec![])` when the file is absent (the cold-start case).
/// Malformed lines bubble up as `Error::Diag` with the
/// `cache-index-malformed` discriminant.
///
/// # Errors
///
/// Propagates I/O failures from reading the file and surfaces parse
/// failures as `cache-index-malformed`.
pub fn read_index(layout: CacheLayout<'_>) -> Result<Vec<CacheIndexEntry>, Error> {
    let path = layout.index_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::Io(err)),
    };
    let mut entries = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: CacheIndexEntry = serde_json::from_str(line).map_err(|err| Error::Diag {
            code: "cache-index-malformed",
            detail: format!("{}:{}: {err}", path.display(), idx + 1),
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

fn digest_dir_name(digest: &str) -> &str {
    // `sha256:<hex>` carries a `:` which is legal but ugly inside a
    // path; strip the prefix so the dir is just the hex digest.
    digest.strip_prefix("sha256:").unwrap_or(digest)
}

#[cfg(test)]
mod tests;
