//! On-disk `<slice_dir>/.metadata.yaml` representation.
//!
//! [`SliceMetadata`] is the document itself; [`PhaseOutcome`] is the
//! latest phase return surface read by `/change:execute`; the
//! [`TouchedSpec`] entries list the specs a slice mutates. The save
//! path goes through [`crate::atomic::yaml_write`] — readers
//! see either the full previous content or the full new content,
//! never a partial write.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_capability::Phase;
use specify_error::Error;

use crate::{Outcome, Rfc3339Stamp};

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
    /// Capability identifier (e.g. `omnia@v1`).
    pub capability: String,
    /// Current lifecycle state.
    pub status: crate::LifecycleStatus,
    /// When the slice was created.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_at: Option<Rfc3339Stamp>,
    /// When the slice entered `Defined`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub defined_at: Option<Rfc3339Stamp>,
    /// When the build phase started.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub build_started_at: Option<Rfc3339Stamp>,
    /// When the slice reached `Complete`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completed_at: Option<Rfc3339Stamp>,
    /// When the slice was merged.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub merged_at: Option<Rfc3339Stamp>,
    /// When the slice was dropped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dropped_at: Option<Rfc3339Stamp>,
    /// Human-readable reason for dropping the slice.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub drop_reason: Option<String>,
    /// Specs affected by this slice.
    #[serde(default)]
    pub touched_specs: Vec<TouchedSpec>,
    /// Latest phase outcome. Written atomically by
    /// `specify slice outcome set` or by `specify_merge::slice::commit` (stamps `Success`
    /// before the archive move). New stamps overwrite; history lives in
    /// `journal.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PhaseOutcome>,
}

/// Result of a phase run (define | build | merge) as recorded in
/// `.metadata.yaml`. Read by `/change:execute` on phase return to
/// decide the next plan transition.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PhaseOutcome {
    /// Which phase produced this outcome.
    pub phase: Phase,
    /// Success, failure, or deferred classification.
    pub outcome: Outcome,
    /// When the outcome was recorded.
    pub at: Rfc3339Stamp,
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
    /// Capability name (kebab-case).
    pub name: String,
    /// Whether this spec is new or modifies an existing baseline.
    #[serde(rename = "type")]
    pub kind: SpecKind,
}

/// Whether a touched spec is new or a modification of an existing
/// baseline.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpecKind {
    /// A brand-new spec not yet in the baseline.
    New,
    /// A modification of an existing baseline spec.
    Modified,
}

impl fmt::Display for SpecKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::New => "new",
            Self::Modified => "modified",
        })
    }
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
    /// [`Error::Yaml`] surfaces serde-saphyr deserialisation failures
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
    /// [`crate::atomic::yaml_write`] fails. The atomicity
    /// envelope is preserved: a failure here leaves any pre-existing
    /// `.metadata.yaml` intact.
    pub fn save(&self, slice_dir: &Path) -> Result<(), Error> {
        let path = Self::path(slice_dir);
        crate::atomic::yaml_write(&path, self)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::{LifecycleStatus, Outcome, Rfc3339Stamp};

    fn sample() -> SliceMetadata {
        SliceMetadata {
            version: METADATA_VERSION,
            capability: "omnia".to_string(),
            status: LifecycleStatus::Building,
            created_at: Some(Rfc3339Stamp::new("2024-08-01T10:00:00Z".to_string())),
            defined_at: Some(Rfc3339Stamp::new("2024-08-01T12:00:00Z".to_string())),
            build_started_at: Some(Rfc3339Stamp::new("2024-08-02T09:30:00Z".to_string())),
            completed_at: Some(Rfc3339Stamp::new("2024-08-03T15:45:00Z".to_string())),
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

    #[test]
    fn load_missing_returns_not_found() {
        let dir = tempdir().expect("tempdir");
        let err = SliceMetadata::load(dir.path()).expect_err("expected error");
        match err {
            Error::ArtifactNotFound { kind, path } => {
                assert_eq!(kind, ".metadata.yaml");
                assert!(
                    path.display().to_string().contains(&dir.path().display().to_string()),
                    "path should include slice dir, got: {}",
                    path.display()
                );
            }
            other => panic!("expected Error::ArtifactNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_malformed_yaml_errors() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(SliceMetadata::path(dir.path()), "not: a\n  valid: yaml\n: structure:")
            .expect("write ok");
        let err = SliceMetadata::load(dir.path()).expect_err("expected error");
        assert!(
            matches!(err, Error::Yaml(_) | Error::Diag { .. }),
            "expected Yaml or Diag error, got {err:?}"
        );
    }

    #[test]
    fn deserializes_yaml_sample() {
        let yaml = r#"capability: omnia
status: building
created-at: "2024-08-01T10:00:00Z"
defined-at: "2024-08-01T12:00:00Z"
build-started-at: "2024-08-02T09:30:00Z"
touched-specs:
  - name: login
    type: modified
  - name: oauth
    type: new
"#;
        let meta: SliceMetadata = serde_saphyr::from_str(yaml).expect("parse ok");
        assert_eq!(meta.status, LifecycleStatus::Building);
        assert_eq!(meta.created_at.as_deref(), Some("2024-08-01T10:00:00Z"));
        assert_eq!(meta.defined_at.as_deref(), Some("2024-08-01T12:00:00Z"));
        assert_eq!(meta.build_started_at.as_deref(), Some("2024-08-02T09:30:00Z"));
        assert_eq!(meta.completed_at, None);
        assert_eq!(meta.touched_specs.len(), 2);
        assert_eq!(meta.touched_specs[0].name, "login");
        assert_eq!(meta.touched_specs[0].kind, SpecKind::Modified);
        assert_eq!(meta.touched_specs[1].name, "oauth");
        assert_eq!(meta.touched_specs[1].kind, SpecKind::New);
    }

    #[test]
    fn serializes_kebab_case_and_lowercase_enums() {
        let meta = SliceMetadata {
            version: METADATA_VERSION,
            capability: "omnia".to_string(),
            status: LifecycleStatus::Building,
            created_at: Some(Rfc3339Stamp::new("2024-08-01T10:00:00Z".to_string())),
            defined_at: None,
            build_started_at: Some(Rfc3339Stamp::new("2024-08-02T09:30:00Z".to_string())),
            completed_at: None,
            merged_at: None,
            dropped_at: None,
            drop_reason: None,
            touched_specs: vec![TouchedSpec {
                name: "login".to_string(),
                kind: SpecKind::Modified,
            }],
            outcome: None,
        };
        let yaml = serde_saphyr::to_string(&meta).expect("serialize ok");
        assert!(yaml.contains("created-at:"), "yaml missing kebab-case created-at:\n{yaml}");
        assert!(yaml.contains("build-started-at:"), "yaml missing build-started-at:\n{yaml}");
        assert!(yaml.contains("touched-specs:"), "yaml missing touched-specs:\n{yaml}");
        assert!(yaml.contains("status: building"), "yaml missing lowercase status:\n{yaml}");
        assert!(yaml.contains("type: modified"), "yaml missing lowercase type:\n{yaml}");
        assert!(!yaml.contains("defined-at:"), "defined-at should be omitted when None:\n{yaml}");
        assert!(
            !yaml.contains("completed-at:"),
            "completed-at should be omitted when None:\n{yaml}"
        );
    }

    #[test]
    fn real_world_slice_file_parses() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Walk both the current `.specify/slices/` layout and the
        // legacy `.specify/changes/` fixture layout so this smoke test
        // keeps working across the on-disk migration.
        let candidates: Vec<PathBuf> = manifest
            .parent()
            .and_then(|p| p.parent())
            .map(|repo_root| {
                vec![
                    repo_root.join(".specify").join(SLICES_DIR_NAME),
                    repo_root.join(".specify").join("changes"),
                ]
            })
            .unwrap_or_default();
        for slices_dir in candidates {
            let Ok(read_dir) = std::fs::read_dir(&slices_dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let slice_path = entry.path();
                if !slice_path.is_dir() {
                    continue;
                }
                if !SliceMetadata::path(&slice_path).exists() {
                    continue;
                }
                SliceMetadata::load(&slice_path).unwrap_or_else(|e| {
                    panic!("existing metadata at {} should parse, got {e:?}", slice_path.display())
                });
                return;
            }
        }
    }

    #[test]
    fn path_appends_metadata_yaml() {
        let dir = Path::new("/tmp/some/slice");
        assert_eq!(SliceMetadata::path(dir), PathBuf::from("/tmp/some/slice/.metadata.yaml"));
    }

    #[test]
    fn spec_type_display_matches_serde() {
        assert_eq!(SpecKind::New.to_string(), "new");
        assert_eq!(SpecKind::Modified.to_string(), "modified");
    }

    /// Back-compat invariant: the implicit pre-v2 metadata schema
    /// (no `version:` field, closed `Outcome`) must round-trip through
    /// the current reader, and the absent version resolves to `1`.
    #[test]
    fn defaults_version_when_absent() {
        let yaml = r#"capability: omnia
status: complete
created-at: "2024-08-01T10:00:00Z"
defined-at: "2024-08-01T12:00:00Z"
build-started-at: "2024-08-02T09:30:00Z"
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
        assert_eq!(meta.status, LifecycleStatus::Complete);
        let stamped = meta.outcome.expect("outcome should round-trip");
        assert_eq!(stamped.phase, Phase::Merge);
        assert_eq!(stamped.outcome, Outcome::Success);
    }

    /// `RegistryAmendmentRequired` round-trips through serde
    /// (write → read → equal) — wire-format proof for the
    /// externally-tagged proposal payload.
    #[test]
    fn registry_amendment_required_round_trips_through_serde() {
        let stamp = Rfc3339Stamp::new("2024-08-04T11:22:33Z".to_string());
        let original = PhaseOutcome {
            phase: Phase::Build,
            outcome: Outcome::RegistryAmendmentRequired {
                proposed_name: "alpha-gateway".to_string(),
                proposed_url: "git@github.com:augentic/alpha-gateway.git".to_string(),
                proposed_capability: "omnia@v1".to_string(),
                proposed_description: Some("Gateway for alpha capability.".to_string()),
                rationale: "build discovered tangled code requiring a split".to_string(),
            },
            at: stamp,
            summary: "registry-amendment-required: alpha-gateway".to_string(),
            context: None,
        };
        let yaml = serde_saphyr::to_string(&original).expect("serialise");
        assert!(
            yaml.contains("registry-amendment-required:"),
            "wire shape must use kebab-case external tag, got:\n{yaml}"
        );
        assert!(
            yaml.contains("proposed-name: alpha-gateway"),
            "wire shape must kebab-case proposal fields, got:\n{yaml}"
        );
        let parsed: PhaseOutcome = serde_saphyr::from_str(&yaml).expect("parse");
        assert_eq!(parsed, original);
    }

    /// A full `.metadata.yaml` carrying the proposal variant must
    /// round-trip end-to-end (load → save → load) — the writer stamps
    /// [`METADATA_VERSION`], the reader accepts it.
    #[test]
    fn metadata_with_registry_amendment_required_round_trips() {
        let dir = tempdir().expect("tempdir");
        let mut meta = sample();
        meta.outcome = Some(PhaseOutcome {
            phase: Phase::Build,
            outcome: Outcome::RegistryAmendmentRequired {
                proposed_name: "alpha-gateway".to_string(),
                proposed_url: "git@github.com:augentic/alpha-gateway.git".to_string(),
                proposed_capability: "omnia@v1".to_string(),
                proposed_description: Some("Gateway for alpha capability.".to_string()),
                rationale: "build discovered tangled code requiring a split".to_string(),
            },
            at: Rfc3339Stamp::new("2024-08-04T11:22:33Z".to_string()),
            summary: "registry-amendment-required: alpha-gateway".to_string(),
            context: None,
        });
        meta.save(dir.path()).expect("save ok");
        let loaded = SliceMetadata::load(dir.path()).expect("load ok");
        assert_eq!(loaded.version, METADATA_VERSION);
        assert_eq!(loaded, meta);
        let on_disk =
            std::fs::read_to_string(SliceMetadata::path(dir.path())).expect("read raw file");
        assert!(
            on_disk.contains(&format!("version: {METADATA_VERSION}")),
            "writer must stamp METADATA_VERSION, got:\n{on_disk}"
        );
        assert!(
            on_disk.contains("registry-amendment-required:"),
            "outcome must use external-tag form, got:\n{on_disk}"
        );
    }
}
