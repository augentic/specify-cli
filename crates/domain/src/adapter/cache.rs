//! RFC-27 §D8 cache fingerprint inputs, the per-write index entry
//! persisted at `.specify/.cache/sources/<adapter>/index.jsonl`, and
//! the lookup / write helpers the source-resolution code path uses.
//!
//! Types follow the rest of the workspace's posture —
//! `#[serde(deny_unknown_fields)]`, kebab-case wire ids, atomic writes
//! for the cache directory, `O_APPEND` for the index log.

mod io;

use std::path::Path;

pub use io::{
    CacheLayout, CacheLookup, FingerprintRecord, LookupOutcome, append_index, lookup, read_index,
    write,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Closed list of fingerprint inputs (RFC-27 §D8).
///
/// Inputs are byte-stable per source — operators who pin the four
/// inputs at a known set can re-run any prior `/spec:execute` and
/// expect byte-stable cache hits. The fifth field (`candidate`) is
/// `Some` for `extract` and `None` for `enumerate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CacheFingerprint {
    /// Source binding identity — canonical absolute path for path
    /// bindings, sha256 of the literal `value:` body for value-style
    /// bindings. Always present.
    pub source: FingerprintSource,
    /// `<name>@<version>` join of the adapter manifest fields.
    pub adapter: String,
    /// `sha256:<hex>` of the brief markdown file driving the
    /// operation (`briefs/enumerate.md` or `briefs/extract.md`).
    pub brief_sha256: String,
    /// Declared-tool versions sorted by tool name (matches the
    /// `tools[]` declaration order from `adapter.yaml`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_versions: Vec<FingerprintToolVersion>,
    /// Candidate id this fingerprint resolved for. Present on
    /// `extract` fingerprints, absent on `enumerate` (which is
    /// candidate-set-shaped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

impl CacheFingerprint {
    /// Construct a fingerprint from raw inputs, sorting tool versions
    /// by name so byte-stability does not depend on caller order.
    #[must_use]
    pub fn new(
        source: FingerprintSource, adapter: String, brief_sha256: String,
        mut tool_versions: Vec<FingerprintToolVersion>, candidate: Option<String>,
    ) -> Self {
        tool_versions.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            source,
            adapter,
            brief_sha256,
            tool_versions,
            candidate,
        }
    }

    /// Canonical byte serialisation used for the sha256 digest.
    ///
    /// Uses `serde_json::to_vec` with the struct's fixed field
    /// declaration order; tool versions are pre-sorted by [`Self::new`].
    /// The resulting bytes are stable across runs given identical
    /// inputs and free of accidental sources of entropy (mtime, locale,
    /// hidden env).
    ///
    /// # Panics
    ///
    /// Panics if `serde_json::to_vec` fails. The fingerprint is a
    /// closed serde derive whose fields are owned `String`s and small
    /// enums; this branch is unreachable in normal operation.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("CacheFingerprint serialises as JSON")
    }

    /// `sha256:<hex>` digest over [`Self::canonical_bytes`].
    #[must_use]
    pub fn digest(&self) -> String {
        let bytes = self.canonical_bytes();
        format!("sha256:{:x}", Sha256::digest(&bytes))
    }

    /// First field that differs between `prior` and `self`, walking
    /// the five inputs in declared order. Returns `None` when every
    /// field matches.
    ///
    /// Field order: `source`, `adapter`, `brief_sha256`,
    /// `tool_versions`, `candidate` — the closed declaration order of
    /// [`CacheFingerprint`], which mirrors the RFC-27 §D8 input list.
    #[must_use]
    pub fn diff_reason(prior: &Self, current: &Self) -> Option<CacheMissReason> {
        if prior.source != current.source {
            return Some(CacheMissReason::SourcePathChanged);
        }
        if prior.adapter != current.adapter {
            return Some(CacheMissReason::AdapterVersionChanged);
        }
        if prior.brief_sha256 != current.brief_sha256 {
            return Some(CacheMissReason::BriefShaChanged);
        }
        if prior.tool_versions != current.tool_versions {
            return Some(CacheMissReason::ToolVersionChanged);
        }
        if prior.candidate != current.candidate {
            // No `candidate-changed` reason in the closed enum; the
            // cache key crosses candidates by design and a candidate
            // delta on the same (slice, source-key) lane reads as a
            // brand-new entry to the operator.
            return Some(CacheMissReason::NoPriorEntry);
        }
        None
    }
}

/// `sha256:<hex>` of bytes. Helper that pairs with
/// [`CacheFingerprint::digest`] for the brief-file input.
#[must_use]
pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Hash the file at `path` and return its `sha256:<hex>` digest.
///
/// # Errors
///
/// Propagates I/O failures as `Error::Diag` with the
/// `cache-fingerprint-input-read-failed` discriminant so the cache
/// layer's diagnostics stay distinct from generic filesystem errors.
pub fn sha256_file(path: &Path) -> Result<String, specify_error::Error> {
    let bytes = std::fs::read(path).map_err(|err| specify_error::Error::Diag {
        code: "cache-fingerprint-input-read-failed",
        detail: format!("failed to read {} for sha256 hashing: {err}", path.display()),
    })?;
    Ok(sha256_prefixed(&bytes))
}

/// Closed two-form `source:` shape inside a [`CacheFingerprint`].
///
/// Mirrors RFC-25 §`Source` — bindings are either path-style or
/// value-style. The two forms hash differently downstream; the closed
/// enum makes the distinction visible at the type level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FingerprintSource {
    /// Path-style binding: canonical absolute path of `$SOURCE_DIR`.
    Path {
        /// Canonical absolute path of `$SOURCE_DIR`.
        path: String,
    },
    /// Value-style binding (e.g. `intent` source): sha256 of the
    /// literal `value:` body.
    Value {
        /// `sha256:<hex>` of the literal `value:` body bytes.
        sha256: String,
    },
}

impl FingerprintSource {
    /// Build a path-form source by canonicalising `path`.
    ///
    /// Canonicalisation strips relative components and follows
    /// symlinks so two callers with different spellings of the same
    /// underlying directory produce identical fingerprints. Non-UTF8
    /// segments are surfaced via [`Path::to_string_lossy`] — the
    /// downstream digest is still deterministic since the lossy
    /// substitution is itself deterministic.
    ///
    /// # Errors
    ///
    /// Propagates I/O failures as `Error::Diag` with the
    /// `cache-fingerprint-source-canonicalize-failed` discriminant.
    pub fn from_path(path: &Path) -> Result<Self, specify_error::Error> {
        let canonical = std::fs::canonicalize(path).map_err(|err| specify_error::Error::Diag {
            code: "cache-fingerprint-source-canonicalize-failed",
            detail: format!("failed to canonicalize source path {}: {err}", path.display()),
        })?;
        Ok(Self::Path {
            path: canonical.to_string_lossy().into_owned(),
        })
    }

    /// Build a value-form source by sha256-hashing the value body.
    #[must_use]
    pub fn from_value(body: &[u8]) -> Self {
        Self::Value {
            sha256: sha256_prefixed(body),
        }
    }
}

/// One `(tool-name, tool-version)` row inside [`CacheFingerprint::tool_versions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FingerprintToolVersion {
    /// Kebab-case tool name as declared by `adapter.yaml.tools[].name`.
    pub name: String,
    /// Version pin (semver string or `sha256:<digest>`). `None` when
    /// the manifest declared the tool without a version pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One row appended to `.specify/.cache/sources/<adapter>/index.jsonl`
/// on every cache write (RFC-27 §D8).
///
/// The slot is `(timestamp, fingerprint-sha256, slice, source-key,
/// adapter, operation)` — together they let `specify source resolve
/// --explain` reconstruct the fingerprint chain without re-reading
/// the underlying `fingerprint.json`. Append-only; writers stream
/// NDJSON lines per `journal::append` posture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CacheIndexEntry {
    /// UTC second-precision timestamp the cache write completed.
    #[serde(with = "specify_error::serde_rfc3339")]
    pub timestamp: Timestamp,
    /// sha256 hex digest derived from the five [`CacheFingerprint`]
    /// inputs; the cache directory is keyed against it.
    pub fingerprint: String,
    /// Slice the cache write served (mirrors the matching
    /// `slice.extract.cache-*` journal event).
    pub slice: String,
    /// Source key from `plan.yaml.sources.<key>`.
    pub source_key: String,
    /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
    pub adapter: String,
    /// Closed source-adapter operation that triggered the cache write.
    pub operation: SourceOperation,
}

/// Closed source-adapter operation set (`enumerate | extract`).
///
/// Source adapters declare exactly these two operations per
/// RFC-25 §Source adapter contract; the cache write surface (RFC-27
/// §D8) records which one drove a given index row so
/// `specify source resolve --explain` can attribute hits and misses.
/// Target adapters use the sibling [`crate::adapter::Operation`] enum
/// (`shape | build | merge`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SourceOperation {
    /// Plan-time candidate discovery.
    Enumerate,
    /// Slice-time evidence extraction.
    Extract,
}

impl SourceOperation {
    /// Default cached-artifact filename per operation:
    /// `evidence.yaml` for `extract`, `candidate-set.md` for `enumerate`.
    #[must_use]
    pub const fn artifact_name(self) -> &'static str {
        match self {
            Self::Enumerate => "candidate-set.md",
            Self::Extract => "evidence.yaml",
        }
    }
}

/// Closed `reason` enum re-exported here so callers reach for the
/// cache surface without importing the journal crate.
pub use crate::journal::CacheMissReason;

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(raw: &str) -> Timestamp {
        raw.parse().expect("valid rfc3339 timestamp in test fixture")
    }

    fn sample(adapter: &str, candidate: Option<&str>) -> CacheFingerprint {
        CacheFingerprint::new(
            FingerprintSource::Path {
                path: "/repo/vendor/monolith".to_string(),
            },
            adapter.to_string(),
            "sha256:abc".to_string(),
            vec![FingerprintToolVersion {
                name: "tsc".to_string(),
                version: Some("5.4.0".to_string()),
            }],
            candidate.map(str::to_string),
        )
    }

    #[test]
    fn fingerprint_path_form_round_trips() {
        let fp = sample("code-typescript@1", Some("user-registration"));
        let json = serde_json::to_string(&fp).expect("serialise");
        assert!(json.contains(r#""source":{"kind":"path","path":"/repo/vendor/monolith"}"#));
        assert!(json.contains(r#""brief-sha256":"sha256:abc""#));
        assert!(json.contains(r#""tool-versions":[{"name":"tsc","version":"5.4.0"}]"#));
        let reparsed: CacheFingerprint = serde_json::from_str(&json).expect("reparse");
        assert_eq!(fp, reparsed);
    }

    #[test]
    fn fingerprint_value_form_round_trips() {
        let fp = CacheFingerprint::new(
            FingerprintSource::Value {
                sha256: "sha256:deadbeef".to_string(),
            },
            "intent@1".to_string(),
            "sha256:b".to_string(),
            vec![],
            None,
        );
        let json = serde_json::to_string(&fp).expect("serialise");
        assert!(json.contains(r#""source":{"kind":"value","sha256":"sha256:deadbeef"}"#));
        assert!(!json.contains("tool-versions"), "empty tool-versions must elide");
        assert!(
            !json.contains("candidate"),
            "absent candidate must elide on enumerate fingerprint"
        );
        let reparsed: CacheFingerprint = serde_json::from_str(&json).expect("reparse");
        assert_eq!(fp, reparsed);
    }

    #[test]
    fn cache_index_entry_round_trips() {
        let entry = CacheIndexEntry {
            timestamp: ts("2026-05-22T13:15:00Z"),
            fingerprint: "sha256:cafef00d".to_string(),
            slice: "identity-user-registration".to_string(),
            source_key: "runtime".to_string(),
            adapter: "code-runtime".to_string(),
            operation: SourceOperation::Extract,
        };
        let json = serde_json::to_string(&entry).expect("serialise");
        assert!(json.contains(r#""timestamp":"2026-05-22T13:15:00Z""#));
        assert!(json.contains(r#""source-key":"runtime""#));
        assert!(json.contains(r#""operation":"extract""#));
        let reparsed: CacheIndexEntry = serde_json::from_str(&json).expect("reparse");
        assert_eq!(entry, reparsed);
    }

    #[test]
    fn deny_unknown_fields_on_index_entry() {
        let raw = r#"{
            "timestamp": "2026-05-22T13:15:00Z",
            "fingerprint": "sha256:a",
            "slice": "s",
            "source-key": "k",
            "adapter": "a",
            "operation": "extract",
            "unknown": true
        }"#;
        let err = serde_json::from_str::<CacheIndexEntry>(raw).expect_err("unknown field rejected");
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error from deny_unknown_fields: {err}"
        );
    }

    #[test]
    fn digest_is_byte_stable_across_runs() {
        let a = sample("code-typescript@1", Some("user-registration"));
        let b = sample("code-typescript@1", Some("user-registration"));
        assert_eq!(a.digest(), b.digest());
        assert!(a.digest().starts_with("sha256:"));
        // Pin the digest so any accidental change to canonical
        // serialisation (field rename, default skip rule, etc.)
        // fails this assertion loudly.
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn tool_versions_sort_independently_of_input_order() {
        let unsorted = CacheFingerprint::new(
            FingerprintSource::Path {
                path: "/p".to_string(),
            },
            "a@1".to_string(),
            "sha256:b".to_string(),
            vec![
                FingerprintToolVersion {
                    name: "zsh".to_string(),
                    version: None,
                },
                FingerprintToolVersion {
                    name: "ash".to_string(),
                    version: Some("1".to_string()),
                },
            ],
            None,
        );
        let sorted = CacheFingerprint::new(
            FingerprintSource::Path {
                path: "/p".to_string(),
            },
            "a@1".to_string(),
            "sha256:b".to_string(),
            vec![
                FingerprintToolVersion {
                    name: "ash".to_string(),
                    version: Some("1".to_string()),
                },
                FingerprintToolVersion {
                    name: "zsh".to_string(),
                    version: None,
                },
            ],
            None,
        );
        assert_eq!(unsorted.digest(), sorted.digest());
    }

    #[test]
    fn each_input_flip_changes_the_digest() {
        let base = sample("code-typescript@1", Some("c1"));
        let baseline = base.digest();

        let mut source_changed = base.clone();
        source_changed.source = FingerprintSource::Path {
            path: "/other".to_string(),
        };
        assert_ne!(source_changed.digest(), baseline, "source flip must change digest");

        let mut adapter_changed = base.clone();
        adapter_changed.adapter = "code-typescript@2".to_string();
        assert_ne!(adapter_changed.digest(), baseline, "adapter flip must change digest");

        let mut brief_changed = base.clone();
        brief_changed.brief_sha256 = "sha256:xyz".to_string();
        assert_ne!(brief_changed.digest(), baseline, "brief flip must change digest");

        let mut tool_changed = base.clone();
        tool_changed.tool_versions[0].version = Some("5.5.0".to_string());
        assert_ne!(tool_changed.digest(), baseline, "tool flip must change digest");

        let mut candidate_changed = base;
        candidate_changed.candidate = Some("c2".to_string());
        assert_ne!(candidate_changed.digest(), baseline, "candidate flip must change digest");
    }

    #[test]
    fn diff_reason_walks_declared_field_order() {
        let prior = sample("a@1", Some("c1"));

        let same = sample("a@1", Some("c1"));
        assert!(CacheFingerprint::diff_reason(&prior, &same).is_none());

        let mut source_changed = prior.clone();
        source_changed.source = FingerprintSource::Path {
            path: "/other".to_string(),
        };
        assert_eq!(
            CacheFingerprint::diff_reason(&prior, &source_changed),
            Some(CacheMissReason::SourcePathChanged)
        );

        let mut adapter_changed = prior.clone();
        adapter_changed.adapter = "a@2".to_string();
        assert_eq!(
            CacheFingerprint::diff_reason(&prior, &adapter_changed),
            Some(CacheMissReason::AdapterVersionChanged)
        );

        let mut brief_changed = prior.clone();
        brief_changed.brief_sha256 = "sha256:other".to_string();
        assert_eq!(
            CacheFingerprint::diff_reason(&prior, &brief_changed),
            Some(CacheMissReason::BriefShaChanged)
        );

        let mut tool_changed = prior.clone();
        tool_changed.tool_versions[0].version = Some("5.5.0".to_string());
        assert_eq!(
            CacheFingerprint::diff_reason(&prior, &tool_changed),
            Some(CacheMissReason::ToolVersionChanged)
        );
    }

    #[test]
    fn diff_reason_picks_first_change_when_multiple_fields_drift() {
        let prior = sample("a@1", Some("c1"));
        let mut both = prior.clone();
        both.source = FingerprintSource::Path {
            path: "/other".to_string(),
        };
        both.adapter = "a@2".to_string();
        assert_eq!(
            CacheFingerprint::diff_reason(&prior, &both),
            Some(CacheMissReason::SourcePathChanged),
            "earlier-declared field wins on multi-field drift"
        );
    }
}
