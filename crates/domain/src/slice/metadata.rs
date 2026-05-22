//! On-disk `<slice_dir>/.metadata.yaml` representation.
//!
//! [`SliceMetadata`] is the document, [`Outcome`] is the latest phase return
//! surface read by `/change:execute`, and [`TouchedSpec`] lists the specs
//! the slice mutates.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::adapter::Operation;
use crate::slice::OutcomeKind;

/// Basename of the slice working directory under `.specify/`.
pub const SLICES_DIR_NAME: &str = "slices";

/// On-disk schema version stamped into new `.metadata.yaml` files.
/// Informational only — readers dispatch on the `outcome` discriminant,
/// not the version number. Pre-v2 files default to `1`.
pub const METADATA_VERSION: u32 = 2;

const fn default_version() -> u32 {
    1
}

/// On-disk representation of `<slice_dir>/.metadata.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SliceMetadata {
    /// On-disk schema version. Defaults to `1` for pre-v2 archives;
    /// current writers stamp [`METADATA_VERSION`]. Readers dispatch on
    /// the outcome discriminant, not this field.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Target-adapter identifier (e.g. `omnia@v1`).
    ///
    /// Renamed from `adapter` in RFC-25 W0.2 — the on-disk and
    /// in-memory field is now `target`. The pre-RFC-25 `adapter`
    /// alias was dropped together with the schema tightening that
    /// shipped in the same change.
    pub target: String,
    /// Current lifecycle state.
    pub status: crate::slice::LifecycleStatus,
    /// When the slice was created.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "specify_error::serde_rfc3339_opt"
    )]
    pub created_at: Option<Timestamp>,
    /// When the slice entered `Refined`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "specify_error::serde_rfc3339_opt"
    )]
    pub defined_at: Option<Timestamp>,
    /// When the slice reached `Built`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "specify_error::serde_rfc3339_opt"
    )]
    pub completed_at: Option<Timestamp>,
    /// When the slice was merged.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "specify_error::serde_rfc3339_opt"
    )]
    pub merged_at: Option<Timestamp>,
    /// When the slice was dropped.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "specify_error::serde_rfc3339_opt"
    )]
    pub dropped_at: Option<Timestamp>,
    /// Human-readable reason for dropping the slice.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub drop_reason: Option<String>,
    /// Specs affected by this slice.
    #[serde(default)]
    pub touched_specs: Vec<TouchedSpec>,
    /// Latest phase outcome. Written atomically by
    /// `crate::merge::slice::commit` (stamps `Success` before the archive move).
    /// History lives in `.specify/journal.jsonl` (RFC-25 §Observability).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

/// Result of a target-adapter operation (shape | build | merge) as
/// recorded in `.metadata.yaml`. Read by `/spec:execute` on phase
/// return to decide the next plan transition.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Outcome {
    /// Which target-adapter operation produced this outcome.
    pub phase: Operation,
    /// Success, failure, or deferred classification. The wire field
    /// name stays `outcome` for back-compat with existing
    /// `.metadata.yaml` files and skill JSON consumers; the Rust name
    /// is `kind` so the `Outcome.outcome` field clash with the enclosing
    /// type is gone.
    #[serde(rename = "outcome")]
    pub kind: OutcomeKind,
    /// When the outcome was recorded.
    #[serde(with = "specify_error::serde_rfc3339")]
    pub at: Timestamp,
    /// Short human-readable summary.
    pub summary: String,
    /// Optional additional context (e.g. stderr output).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// One entry in [`SliceMetadata::touched_specs`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TouchedSpec {
    /// Adapter name (kebab-case).
    pub name: String,
    /// Whether this spec is new or modifies an existing baseline.
    #[serde(rename = "type")]
    pub kind: SpecKind,
}

/// Whether a touched spec is new or a modification of an existing
/// baseline.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum SpecKind {
    /// A brand-new spec not yet in the baseline.
    New,
    /// A modification of an existing baseline spec.
    Modified,
}

impl SliceMetadata {
    /// Convenience helper: `<slice_dir>/.metadata.yaml`.
    #[must_use]
    pub fn path(slice_dir: &Path) -> PathBuf {
        slice_dir.join(".metadata.yaml")
    }

    /// Load `.metadata.yaml` from a slice directory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ArtifactNotFound`] (`kind = ".metadata.yaml"`)
    /// when the file is absent — the canonical "not a slice directory"
    /// signal that `specify slice list` and `/change:execute` rely on.
    /// [`Error::YamlDe`] surfaces serde-saphyr deserialisation failures
    /// (malformed YAML, unknown enum tags, type mismatches);
    /// [`Error::Io`] propagates filesystem read errors past the
    /// existence probe (permissions, mid-flight truncation).
    pub fn load(slice_dir: &Path) -> Result<Self, Error> {
        let path = Self::path(slice_dir);
        if !path.exists() {
            return Err(Error::ArtifactNotFound {
                kind: ".metadata.yaml",
                path,
            });
        }
        let content = std::fs::read_to_string(&path)?;
        let meta: Self = serde_saphyr::from_str(&content)?;
        Ok(meta)
    }

    /// Atomically write `.metadata.yaml` to a slice directory,
    /// overwriting if present. Always trailing-newlined.
    ///
    /// # Errors
    ///
    /// Returns [`Error::YamlSer`] when serde-saphyr fails to encode
    /// `self` — typically a serializer bug rather than a data issue,
    /// since every field of [`SliceMetadata`] is YAML-safe by
    /// construction. Returns [`Error::Io`] when the temp-file create /
    /// write / `sync_all` / atomic rename in
    /// [`crate::slice::atomic::yaml_write`] fails. The atomicity
    /// envelope is preserved: a failure here leaves any pre-existing
    /// `.metadata.yaml` intact.
    pub fn save(&self, slice_dir: &Path) -> Result<(), Error> {
        let path = Self::path(slice_dir);
        crate::slice::atomic::yaml_write(&path, self)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::slice::{LifecycleStatus, OutcomeKind};

    fn parse_stamp(raw: &str) -> Timestamp {
        raw.parse().expect("valid rfc3339 timestamp in test fixture")
    }

    fn sample() -> SliceMetadata {
        SliceMetadata {
            version: METADATA_VERSION,
            target: "omnia".to_string(),
            status: LifecycleStatus::Refined,
            created_at: Some(parse_stamp("2024-08-01T10:00:00Z")),
            defined_at: Some(parse_stamp("2024-08-01T12:00:00Z")),
            completed_at: Some(parse_stamp("2024-08-03T15:45:00Z")),
            merged_at: None,
            dropped_at: None,
            drop_reason: None,
            touched_specs: vec![
                TouchedSpec {
                    name: "login".to_string(),
                    kind: SpecKind::Modified,
                },
                TouchedSpec {
                    name: "oauth".to_string(),
                    kind: SpecKind::New,
                },
            ],
            outcome: None,
        }
    }

    #[test]
    fn save_load_round_trips() {
        let dir = tempdir().expect("tempdir");
        let meta = sample();
        meta.save(dir.path()).expect("save ok");
        let loaded = SliceMetadata::load(dir.path()).expect("load ok");
        assert_eq!(loaded, meta);
    }

    /// Back-compat invariant: the implicit pre-v2 metadata schema
    /// (no `version:` field, closed `OutcomeKind`) must round-trip
    /// through the current reader, and the absent version resolves to
    /// `1`.
    #[test]
    fn defaults_version_when_absent() {
        let yaml = r#"target: omnia
status: built
created-at: "2024-08-01T10:00:00Z"
defined-at: "2024-08-01T12:00:00Z"
completed-at: "2024-08-03T15:45:00Z"
touched-specs:
  - name: login
    type: modified
outcome:
  phase: merge
  outcome: success
  at: "2024-08-03T15:45:00Z"
  summary: "Baseline updated."
"#;
        let meta: SliceMetadata =
            serde_saphyr::from_str(yaml).expect("parse legacy v1 metadata file");
        assert_eq!(meta.version, 1, "absent version should default to 1");
        assert_eq!(meta.status, LifecycleStatus::Built);
        let stamped = meta.outcome.expect("outcome should round-trip");
        assert_eq!(stamped.phase, Operation::Merge);
        assert_eq!(stamped.kind, OutcomeKind::Success);
    }
}
