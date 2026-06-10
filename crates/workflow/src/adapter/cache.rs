//! extraction cache fingerprint contract cache fingerprint inputs and the per-write index entry
//! persisted at `.specify/cache/extractions/<adapter>/index.jsonl`.
//!
//! Lookup / write helpers the source-resolution code path uses.
//!
//! Types follow the rest of the workspace's posture —
//! `#[serde(deny_unknown_fields)]`, kebab-case wire ids, atomic writes
//! for the cache directory, `O_APPEND` for the index log.

mod io;

use std::path::Path;

pub use io::{CacheLayout, CacheLookup, LookupOutcome, append_index, lookup, read_index, write};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
/// Closed list of fingerprint inputs (extraction cache fingerprint contract).
///
/// Inputs are byte-stable per source — operators who pin the four
/// inputs at a known set can re-run any prior `/spec:execute` and
/// expect byte-stable cache hits. The fifth field (`lead`) is
/// `Some` for `extract` and `None` for `survey`.
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
    /// operation (`briefs/survey.md` or `briefs/extract.md`).
    pub brief_sha256: String,
    /// Declared-tool versions sorted by tool name (matches the
    /// `tools[]` declaration order from `adapter.yaml`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_versions: Vec<FingerprintToolVersion>,
    /// Candidate id this fingerprint resolved for. Present on
    /// `extract` fingerprints, absent on `survey` (which is
    /// lead-set-shaped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
}

impl CacheFingerprint {
    /// Construct a fingerprint from raw inputs, sorting tool versions
    /// by name so byte-stability does not depend on caller order.
    #[must_use]
    pub fn new(
        source: FingerprintSource, adapter: String, brief_sha256: String,
        mut tool_versions: Vec<FingerprintToolVersion>, lead: Option<String>,
    ) -> Self {
        tool_versions.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            source,
            adapter,
            brief_sha256,
            tool_versions,
            lead,
        }
    }

    /// Canonical byte serialisation used for the sha256 digest.
    ///
    /// Uses `serde_json::to_vec` with the struct's fixed field
    /// declaration order; tool versions are pre-sorted by [`Self::new`].
    /// The resulting bytes are stable across runs given identical
    /// inputs and free of accidental sources of entropy (mtime, locale,
    /// hidden env).
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // `CacheFingerprint` is a closed serde derive over owned `String`s and
        // small enums with no map keys or floats, so `to_vec` cannot fail;
        // `unreachable!` keeps digest stability from depending on `expect`'s
        // panic-message policy (REVIEW.md A3).
        serde_json::to_vec(self)
            .unwrap_or_else(|_| unreachable!("CacheFingerprint is infallibly JSON-serialisable"))
    }

    /// `sha256:<hex>` digest over [`Self::canonical_bytes`].
    #[must_use]
    pub fn digest(&self) -> String {
        let bytes = self.canonical_bytes();
        format!("sha256:{}", specify_digest::sha256_hex(&bytes))
    }

    /// First field that differs between `prior` and `self`, walking
    /// the five inputs in declared order. Returns `None` when every
    /// field matches.
    ///
    /// Field order: `source`, `adapter`, `brief_sha256`,
    /// `tool_versions`, `lead` — the closed declaration order of
    /// [`CacheFingerprint`], which mirrors the extraction cache fingerprint contract input list.
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
        if prior.lead != current.lead {
            // No `lead-changed` reason in the closed enum; the
            // cache key crosses leads by design and a lead
            // delta on the same (slice, source) lane reads as a
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
    format!("sha256:{}", specify_digest::sha256_hex(bytes))
}

/// Closed two-form `source:` shape inside a [`CacheFingerprint`].
///
/// Mirrors workflow §`Source` — bindings are either path-style or
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

/// One row appended to `.specify/cache/extractions/<adapter>/index.jsonl`
/// on every cache write (extraction cache fingerprint contract).
///
/// The row carries the full closed input record alongside the digest,
/// so `specify source resolve --explain` and the miss-reason
/// classifier read the fingerprint chain from the index alone — the
/// cache entry directory holds only the artifact. Append-only; writers
/// stream NDJSON lines per `journal::append_batch` posture.
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
    pub source: String,
    /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
    pub adapter: String,
    /// Closed source-adapter operation that triggered the cache write.
    pub operation: SourceOperation,
    /// Closed five-input record behind [`Self::fingerprint`]. `None`
    /// reads as no-prior-entry to the miss-reason classifier (the
    /// cache posture is "warn and treat as miss", never fail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<CacheFingerprint>,
}

/// Closed source-adapter operation set re-exported from the shared
/// `adapter::operation` module so cache consumers reach the type via
/// the same import they use for [`CacheIndexEntry`].
///
/// The cache write surface (extraction cache fingerprint contract) records which one drove a
/// given index row so `specify source resolve --explain` can
/// attribute hits and misses. Target adapters use the sibling
/// [`TargetOperation`](crate::adapter::TargetOperation) enum
/// (`shape | build | merge`).
pub use crate::adapter::operation::SourceOperation;
/// Closed `reason` enum re-exported here so callers reach for the
/// cache surface without importing the journal crate.
pub use crate::journal::CacheMissReason;

#[cfg(test)]
mod tests;
