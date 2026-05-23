//! RFC-27 §D8 cache lookup / write / index helpers.
//!
//! Cache directory layout (RFC-27 §D8):
//!
//! ```text
//! .specify/.cache/extractions/<adapter>/
//!     <fingerprint>/
//!         evidence.yaml      # or candidate-set.md for enumerate
//!         fingerprint.json   # full input record for audit
//!     index.jsonl            # one row per cache write; append-only
//! ```
//!
//! The extraction cache is per-adapter only (not per-axis) — only
//! source adapters extract — and lives in its own root, disjoint from
//! the per-axis manifest cache at
//! `.specify/.cache/manifests/{sources,targets}/<name>/`. See
//! [DECISIONS.md §"Cache layout"].
//!
//! Atomic writes mirror [DECISIONS.md §"Atomic writes"]: the cache
//! directory body uses [`bytes_write`] (tempfile + rename), while the
//! per-row index appends go through `O_APPEND` exactly like
//! `journal::append_batch`.
//!
//! [DECISIONS.md §"Atomic writes"]: ../../../../DECISIONS.md#atomic-writes
//! [DECISIONS.md §"Cache layout"]: ../../../../DECISIONS.md#cache-layout
//! [`bytes_write`]: crate::slice::atomic::bytes_write

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::adapter::CacheMode;
use crate::adapter::cache::{CacheFingerprint, CacheIndexEntry, CacheMissReason, SourceOperation};
use crate::adapter::core::EXTRACTIONS_CACHE_DIR;
use crate::slice::atomic::bytes_write;

const INDEX_FILE_NAME: &str = "index.jsonl";
const FINGERPRINT_RECORD_NAME: &str = "fingerprint.json";

/// Filesystem coordinates for the RFC-27 §D8 cache scoped to one
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

    /// `.specify/.cache/extractions/<adapter>/`.
    #[must_use]
    pub fn adapter_dir(&self) -> PathBuf {
        self.project_dir
            .join(".specify")
            .join(".cache")
            .join(EXTRACTIONS_CACHE_DIR)
            .join(self.adapter)
    }

    /// `.specify/.cache/extractions/<adapter>/<fingerprint-sha256>/`.
    #[must_use]
    pub fn fingerprint_dir(&self, digest: &str) -> PathBuf {
        self.adapter_dir().join(digest_dir_name(digest))
    }

    /// `.specify/.cache/extractions/<adapter>/<fp>/fingerprint.json`.
    #[must_use]
    pub fn fingerprint_record_path(&self, digest: &str) -> PathBuf {
        self.fingerprint_dir(digest).join(FINGERPRINT_RECORD_NAME)
    }

    /// `.specify/.cache/extractions/<adapter>/<fp>/<artifact-name>`.
    #[must_use]
    pub fn artifact_path(&self, digest: &str, artifact_name: &str) -> PathBuf {
        self.fingerprint_dir(digest).join(artifact_name)
    }

    /// `.specify/.cache/extractions/<adapter>/index.jsonl`.
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
    /// Closed input record per RFC-27 §D8.
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
    /// `.specify/.cache/extractions/<adapter>/<fp>/` regardless of hit /
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
///    the same `(slice, source-key, operation)` lane, load that
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
    slice: &str, source_key: &str, operation: SourceOperation,
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

    let reason = miss_reason(layout, fingerprint, slice, source_key, operation)?;
    Ok(CacheLookup {
        digest,
        cache_dir,
        outcome: LookupOutcome::Miss { reason },
    })
}

fn miss_reason(
    layout: CacheLayout<'_>, fingerprint: &CacheFingerprint, slice: &str, source_key: &str,
    operation: SourceOperation,
) -> Result<CacheMissReason, Error> {
    let entries = read_index(layout)?;
    let Some(prior) = entries
        .into_iter()
        .rev()
        .find(|e| e.slice == slice && e.source_key == source_key && e.operation == operation)
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
        // Cache-corruption — RFC-27 §D8 leans on "warn and treat as
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
/// `.specify/.cache/extractions/<adapter>/index.jsonl`.
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
mod tests {
    use jiff::Timestamp;
    use tempfile::tempdir;

    use super::*;
    use crate::adapter::cache::{FingerprintSource, FingerprintToolVersion};

    fn ts(raw: &str) -> Timestamp {
        raw.parse().expect("valid rfc3339 timestamp in test fixture")
    }

    fn fp(adapter: &str) -> CacheFingerprint {
        CacheFingerprint::new(
            FingerprintSource::Path {
                path: "/repo/legacy".to_string(),
            },
            adapter.to_string(),
            "sha256:brief".to_string(),
            vec![FingerprintToolVersion {
                name: "tsc".to_string(),
                version: Some("5.4.0".to_string()),
            }],
            Some("user-registration".to_string()),
        )
    }

    fn index_entry(layout_adapter: &str, digest: &str) -> CacheIndexEntry {
        CacheIndexEntry {
            timestamp: ts("2026-05-22T13:15:00Z"),
            fingerprint: digest.to_string(),
            slice: "identity-user-registration".to_string(),
            source_key: "legacy".to_string(),
            adapter: layout_adapter.to_string(),
            operation: SourceOperation::Extract,
        }
    }

    #[test]
    fn write_then_lookup_is_a_hit() {
        let dir = tempdir().expect("tempdir");
        let layout = CacheLayout::new(dir.path(), "code-typescript");
        let fingerprint = fp("code-typescript@1");
        let digest = fingerprint.digest();
        let entry = index_entry("code-typescript", &digest);

        // Cold-start: miss with no-prior-entry.
        let cold =
            lookup(layout, &fingerprint, None, &entry.slice, &entry.source_key, entry.operation)
                .expect("cold lookup");
        assert!(matches!(
            cold.outcome,
            LookupOutcome::Miss {
                reason: CacheMissReason::NoPriorEntry
            }
        ));

        write(layout, &fingerprint, b"---\nclaims: []\n", "evidence.yaml", None, &entry)
            .expect("write");

        let warm =
            lookup(layout, &fingerprint, None, &entry.slice, &entry.source_key, entry.operation)
                .expect("warm lookup");
        match warm.outcome {
            LookupOutcome::Hit { cache_dir } => {
                assert!(cache_dir.is_dir(), "hit cache_dir must exist: {}", cache_dir.display());
                assert!(cache_dir.join("evidence.yaml").is_file(), "artifact persisted");
                assert!(cache_dir.join("fingerprint.json").is_file(), "record persisted");
            }
            LookupOutcome::Miss { reason } => panic!("expected Hit, got Miss({reason})"),
        }
        let entries = read_index(layout).expect("read index");
        assert_eq!(entries.len(), 1, "one row per cache write");
        assert_eq!(entries[0].fingerprint, digest);
    }

    #[test]
    fn adapter_opt_out_misses_without_writing_dir() {
        let dir = tempdir().expect("tempdir");
        let layout = CacheLayout::new(dir.path(), "doc");
        let fingerprint = fp("doc@1");
        let digest = fingerprint.digest();
        let entry = index_entry("doc", &digest);

        let outcome = lookup(
            layout,
            &fingerprint,
            Some(CacheMode::OptOut),
            &entry.slice,
            &entry.source_key,
            entry.operation,
        )
        .expect("opt-out lookup");
        assert!(matches!(
            outcome.outcome,
            LookupOutcome::Miss {
                reason: CacheMissReason::AdapterOptOut
            }
        ));

        write(layout, &fingerprint, b"unused", "evidence.yaml", Some(CacheMode::OptOut), &entry)
            .expect("opt-out write still appends index");
        assert!(
            !layout.fingerprint_dir(&digest).exists(),
            "opt-out must not create the cache directory"
        );
        let entries = read_index(layout).expect("read index");
        assert_eq!(entries.len(), 1, "index still records the audit row under opt-out");
    }

    #[test]
    fn adapter_version_bump_reports_changed_reason() {
        let dir = tempdir().expect("tempdir");
        let layout = CacheLayout::new(dir.path(), "code-typescript");
        let v1 = fp("code-typescript@1");
        let v2 = fp("code-typescript@2");
        let entry_v1 = index_entry("code-typescript", &v1.digest());

        write(layout, &v1, b"e1", "evidence.yaml", None, &entry_v1).expect("write v1");

        let outcome =
            lookup(layout, &v2, None, &entry_v1.slice, &entry_v1.source_key, entry_v1.operation)
                .expect("v2 lookup");
        match outcome.outcome {
            LookupOutcome::Miss { reason } => {
                assert_eq!(reason, CacheMissReason::AdapterVersionChanged);
            }
            LookupOutcome::Hit { cache_dir } => {
                panic!("expected miss, got Hit({})", cache_dir.display())
            }
        }
    }

    #[test]
    fn corrupt_prior_record_is_treated_as_no_prior_entry() {
        let dir = tempdir().expect("tempdir");
        let layout = CacheLayout::new(dir.path(), "code-typescript");
        let prior = fp("code-typescript@1");
        let entry = index_entry("code-typescript", &prior.digest());
        write(layout, &prior, b"e1", "evidence.yaml", None, &entry).expect("write");

        // Corrupt the prior fingerprint.json.
        let record_path = layout.fingerprint_record_path(&prior.digest());
        std::fs::write(&record_path, "{not json").expect("clobber record");

        let next = fp("code-typescript@2");
        let outcome = lookup(layout, &next, None, &entry.slice, &entry.source_key, entry.operation)
            .expect("lookup on corrupt prior");
        assert!(matches!(
            outcome.outcome,
            LookupOutcome::Miss {
                reason: CacheMissReason::NoPriorEntry
            }
        ));
    }

    #[test]
    fn index_read_skips_blank_lines_and_rejects_garbage() {
        let dir = tempdir().expect("tempdir");
        let layout = CacheLayout::new(dir.path(), "code-typescript");
        std::fs::create_dir_all(layout.adapter_dir()).expect("mkdir");
        std::fs::write(
            layout.index_path(),
            "{\"timestamp\":\"2026-05-22T13:15:00Z\",\"fingerprint\":\"sha256:a\",\"slice\":\"s\",\"source-key\":\"k\",\"adapter\":\"a\",\"operation\":\"extract\"}\n\n",
        )
        .expect("write index");
        let rows = read_index(layout).expect("read index");
        assert_eq!(rows.len(), 1);

        std::fs::write(layout.index_path(), "garbage\n").expect("clobber");
        let err = read_index(layout).expect_err("malformed index");
        match err {
            Error::Diag { code, .. } => assert_eq!(code, "cache-index-malformed"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn digest_dir_name_strips_sha256_prefix() {
        assert_eq!(digest_dir_name("sha256:abc"), "abc");
        assert_eq!(digest_dir_name("abc"), "abc");
    }
}
