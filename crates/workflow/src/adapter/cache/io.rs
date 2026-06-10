//! extraction cache fingerprint contract cache lookup / write / index helpers.
//!
//! Cache directory layout (extraction cache fingerprint contract):
//!
//! ```text
//! .specify/cache/extractions/<adapter>/
//!     <fingerprint>/
//!         evidence.yaml      # or leads.md for survey
//!     index.jsonl            # one row per cache write; append-only
//! ```
//!
//! Each `index.jsonl` row carries the full closed fingerprint input
//! record ([`CacheIndexEntry::inputs`]), so the entry directory holds
//! only the artifact and miss-reason classification reads the index
//! alone. The index is pure cache mechanism, not an audit log: it
//! exists to serve the miss-reason classifier, durable audit is the
//! journal's job, and an opt-out adapter writes nothing here at all
//! (the caller skips [`write`] entirely).
//!
//! The extraction cache is per-adapter only (not per-axis) — only
//! source adapters extract — and lives in its own root, disjoint from
//! the per-axis manifest cache at
//! `.specify/cache/manifests/{sources,targets}/<name>/`. Per-operation
//! agent scratch lanes live outside the cache tree altogether, under
//! `.specify/scratch/<adapter>/{survey,<slice>}/`
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

use specify_error::Error;
use specify_model::atomic::bytes_write;

use crate::adapter::CacheMode;
use crate::adapter::cache::{CacheFingerprint, CacheIndexEntry, CacheMissReason, SourceOperation};
use crate::adapter::core::EXTRACTIONS_CACHE_DIR;

const INDEX_FILE_NAME: &str = "index.jsonl";

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

/// Outcome of [`lookup`] — either a hit on the cache directory or a
/// miss carrying the closed [`CacheMissReason`] discriminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupOutcome {
    /// Cache directory holds the operation's artifact for the current
    /// input digest.
    Hit {
        /// Path to the `<fp>/` directory containing the cached
        /// artifact.
        cache_dir: PathBuf,
    },
    /// No cached artifact for the digest, or the adapter declared
    /// `cache: opt-out`.
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
/// 2. `<adapter>/<digest>/<artifact-name>` exists → hit.
/// 3. Otherwise, scan `index.jsonl` for the most recent prior entry on
///    the same `(slice, source, operation)` lane and diff that row's
///    [`CacheIndexEntry::inputs`] field-by-field per
///    [`CacheFingerprint::diff_reason`].
/// 4. No prior entry (or a prior row carrying no input record) →
///    [`CacheMissReason::NoPriorEntry`].
///
/// # Errors
///
/// Propagates I/O failures reading the index log. A prior row without
/// an input record is **not** an error — the miss is reported as
/// [`CacheMissReason::NoPriorEntry`] and the operator can rebuild the
/// cache.
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

    if layout.artifact_path(&digest, operation.artifact_name()).is_file() {
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

    // A row without an input record allows no diff — the extraction
    // cache leans on "warn and treat as miss" rather than failing the
    // whole operation.
    let Some(prior_inputs) = prior.inputs else {
        return Ok(CacheMissReason::NoPriorEntry);
    };
    Ok(CacheFingerprint::diff_reason(&prior_inputs, fingerprint)
        .unwrap_or(CacheMissReason::NoPriorEntry))
}

/// Write a cache entry: artifact bytes plus an `index.jsonl` row.
///
/// Unconditional — opt-out is the caller's branch: an adapter with an
/// effective `cache: opt-out` never reaches this function (the journal's
/// cache-miss event with `reason: adapter-opt-out` is the only trace),
/// so the extraction tree holds entries for caching adapters only.
///
/// # Errors
///
/// Propagates I/O failures from the directory create, atomic
/// tempfile-rename of the artifact, and the index append.
pub fn write(
    layout: CacheLayout<'_>, fingerprint: &CacheFingerprint, artifact_bytes: &[u8],
    artifact_name: &str, entry: &CacheIndexEntry,
) -> Result<(), Error> {
    let artifact_path = layout.artifact_path(&fingerprint.digest(), artifact_name);
    bytes_write(&artifact_path, artifact_bytes)?;
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
