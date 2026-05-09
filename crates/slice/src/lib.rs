//! Slice `.metadata.yaml` document and lifecycle state machine.
//!
//! Exposes [`SliceMetadata`] and the [`LifecycleStatus`] graph between
//! `Defining`, `Defined`, `Building`, `Complete`, `Merged`, `Dropped`.
//! Verb-level operations live in [`actions`].

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use specify_capability::Phase;
use specify_error::Error;

/// Verb-level operations on a Specify slice directory.
pub mod actions;
/// Crash-safe write helpers shared with `specify-change`.
pub mod atomic;
/// On-disk journal for append-only audit logging.
pub mod journal;
/// RFC 3339 timestamp newtype.
pub mod timestamp;

pub use actions::{CreateIfExists, CreateOutcome, Overlap, format_rfc3339};
pub use journal::{EntryKind, Journal, JournalEntry};
pub use timestamp::Rfc3339Stamp;

/// Basename of the slice working directory under `.specify/`.
pub const SLICES_DIR_NAME: &str = "slices";

/// On-disk schema version stamped into new `.metadata.yaml` files.
/// Informational only — readers dispatch on the `outcome` discriminant,
/// not the version number. Pre-v2 files default to `1`.
pub const METADATA_VERSION: u32 = 2;

const fn default_metadata_version() -> u32 {
    1
}

/// On-disk representation of `<slice_dir>/.metadata.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SliceMetadata {
    /// On-disk schema version. Defaults to `1` for pre-v2 archives; current
    /// writers stamp [`METADATA_VERSION`]. Readers dispatch on the outcome
    /// discriminant, not this field.
    #[serde(default = "default_metadata_version")]
    pub version: u32,
    /// Capability identifier stored in this slice's on-disk `schema` field.
    pub schema: String,
    /// Current lifecycle state.
    pub status: LifecycleStatus,
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
    /// Latest phase outcome. Written atomically by `specify slice outcome set`
    /// or by `merge_slice` (stamps `Success` before the archive move). New
    /// stamps overwrite; history lives in `journal.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PhaseOutcome>,
}

/// Result of a phase run (define | build | merge) as recorded in
/// `.metadata.yaml`. Read by `/change:execute` on phase return to decide
/// the next plan transition.
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

/// Phase outcome reported to `/change:execute`. Unit variants serialise as
/// `outcome: success` etc.; [`Self::RegistryAmendmentRequired`] is an
/// externally-tagged map carrying its proposal payload.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", rename_all_fields = "kebab-case")]
#[non_exhaustive]
pub enum Outcome {
    /// Phase completed successfully.
    Success,
    /// Phase failed.
    Failure,
    /// Phase deferred (needs human input).
    Deferred,
    /// Phase blocked pending a registry amendment. `/change:execute` treats
    /// this like `deferred` and surfaces the proposal payload to the operator.
    RegistryAmendmentRequired {
        /// Kebab-case project name proposed for the registry.
        proposed_name: String,
        /// Clone URL for the proposed project (git remote / ssh / http(s) /
        /// `git+...`). Same shape rules as `specify registry add --url`.
        proposed_url: String,
        /// Capability identifier (e.g. `omnia@v1`).
        proposed_schema: String,
        /// Optional human-readable description of the proposed project.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proposed_description: Option<String>,
        /// Free-form rationale, surfaced verbatim to the operator.
        rationale: String,
    },
}

/// Lifecycle states a slice passes through.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LifecycleStatus {
    /// Slice is being defined (artifacts authored).
    Defining,
    /// Definition complete, awaiting build.
    Defined,
    /// Build phase in progress.
    Building,
    /// Build complete, awaiting merge.
    Complete,
    /// Specs merged into baseline and slice archived.
    Merged,
    /// Slice discarded without merging.
    Dropped,
}

/// One entry in `SliceMetadata::touched_specs`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TouchedSpec {
    /// Capability name (kebab-case).
    pub name: String,
    /// Whether this spec is new or modifies an existing baseline.
    #[serde(rename = "type")]
    pub kind: SpecKind,
}

/// Whether a touched spec is new or a modification of an existing baseline.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpecKind {
    /// A brand-new spec not yet in the baseline.
    New,
    /// A modification of an existing baseline spec.
    Modified,
}

impl fmt::Display for LifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Defining => "defining",
            Self::Defined => "defined",
            Self::Building => "building",
            Self::Complete => "complete",
            Self::Merged => "merged",
            Self::Dropped => "dropped",
        })
    }
}

impl fmt::Display for Outcome {
    /// Renders the kebab-case discriminant; payload fields are emitted via
    /// serde when callers need the structured shape.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.discriminant())
    }
}

impl Outcome {
    /// Kebab-case discriminant matching the on-disk serde tag.
    #[must_use]
    pub const fn discriminant(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Deferred => "deferred",
            Self::RegistryAmendmentRequired { .. } => "registry-amendment-required",
        }
    }
}

impl fmt::Display for SpecKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::New => "new",
            Self::Modified => "modified",
        })
    }
}

impl LifecycleStatus {
    /// The creation edge (`START → Defining`). Called by `init`/`define`.
    #[must_use]
    pub const fn initial() -> Self {
        Self::Defining
    }

    /// Whether this status is terminal (no further transitions possible).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Merged | Self::Dropped)
    }

    /// Whether `self → target` is a legal edge in the lifecycle state machine.
    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        use LifecycleStatus::{Building, Complete, Defined, Defining, Dropped, Merged};
        matches!(
            (self, target),
            (Defining, Defined | Complete)
                | (Defined, Defining | Building)
                | (Building, Complete)
                | (Complete, Merged)
                | (Defining | Defined | Building | Complete, Dropped)
        )
    }

    /// Attempt a transition from `self` to `target`.
    ///
    /// # Errors
    ///
    /// Returns `Error::Lifecycle` if the transition is illegal.
    pub fn transition(self, target: Self) -> Result<Self, Error> {
        if self.can_transition_to(target) {
            Ok(target)
        } else {
            Err(Error::Lifecycle {
                expected: format!("valid transition from {self:?}"),
                found: format!("{target:?}"),
            })
        }
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
    /// `Error::ArtifactNotFound` if missing, `Error::Yaml` if malformed,
    /// `Error::Io` on other read failures.
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

    /// Atomically write `.metadata.yaml` to a slice directory, overwriting
    /// if present. Always trailing-newlined.
    ///
    /// # Errors
    ///
    /// `Error::Io` on write failure, `Error::Yaml` on serialisation failure.
    pub fn save(&self, slice_dir: &Path) -> Result<(), Error> {
        let path = Self::path(slice_dir);
        crate::atomic::atomic_yaml_write(&path, self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tempfile::tempdir;

    use super::*;

    const ALL_STATUSES: [LifecycleStatus; 6] = [
        LifecycleStatus::Defining,
        LifecycleStatus::Defined,
        LifecycleStatus::Building,
        LifecycleStatus::Complete,
        LifecycleStatus::Merged,
        LifecycleStatus::Dropped,
    ];

    fn allowed_edges() -> HashSet<(LifecycleStatus, LifecycleStatus)> {
        use LifecycleStatus::*;
        let mut set = HashSet::new();
        set.insert((Defining, Defined));
        set.insert((Defined, Defining));
        set.insert((Defined, Building));
        set.insert((Building, Complete));
        set.insert((Complete, Merged));
        set.insert((Defining, Complete));
        // `any non-terminal -> Dropped`
        set.insert((Defining, Dropped));
        set.insert((Defined, Dropped));
        set.insert((Building, Dropped));
        set.insert((Complete, Dropped));
        set
    }

    #[test]
    fn initial_is_defining() {
        assert_eq!(LifecycleStatus::initial(), LifecycleStatus::Defining);
    }

    #[test]
    fn terminal_states_are_terminal() {
        assert!(LifecycleStatus::Merged.is_terminal());
        assert!(LifecycleStatus::Dropped.is_terminal());
        assert!(!LifecycleStatus::Defining.is_terminal());
        assert!(!LifecycleStatus::Defined.is_terminal());
        assert!(!LifecycleStatus::Building.is_terminal());
        assert!(!LifecycleStatus::Complete.is_terminal());
    }

    #[test]
    fn transition_table_matches_oracle() {
        let allowed = allowed_edges();
        for &from in &ALL_STATUSES {
            for &to in &ALL_STATUSES {
                let expected = allowed.contains(&(from, to));
                let actual = from.can_transition_to(to);
                assert_eq!(
                    actual, expected,
                    "({from:?}) -> ({to:?}): expected allowed={expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn terminal_states_no_outgoing_edges() {
        for &from in &ALL_STATUSES {
            if !from.is_terminal() {
                continue;
            }
            for &to in &ALL_STATUSES {
                assert!(
                    !from.can_transition_to(to),
                    "terminal state {from:?} must not allow -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn legal_edges_round_trip() {
        for (from, to) in allowed_edges() {
            let result = from
                .transition(to)
                .unwrap_or_else(|e| panic!("expected {from:?} -> {to:?} to succeed, got {e:?}"));
            assert_eq!(result, to);
        }
    }

    #[test]
    fn illegal_edges_return_lifecycle_error() {
        let allowed = allowed_edges();
        for &from in &ALL_STATUSES {
            for &to in &ALL_STATUSES {
                if allowed.contains(&(from, to)) {
                    continue;
                }
                let err = from
                    .transition(to)
                    .expect_err(&format!("{from:?} -> {to:?} should be rejected"));
                match err {
                    Error::Lifecycle { expected, found } => {
                        let from_dbg = format!("{from:?}");
                        let to_dbg = format!("{to:?}");
                        assert!(
                            expected.contains(&from_dbg),
                            "expected message {expected:?} should mention {from_dbg:?}"
                        );
                        assert!(
                            found.contains(&to_dbg),
                            "found message {found:?} should mention {to_dbg:?}"
                        );
                    }
                    other => panic!("expected Error::Lifecycle, got {other:?}"),
                }
            }
        }
    }

    fn sample_metadata() -> SliceMetadata {
        SliceMetadata {
            version: METADATA_VERSION,
            schema: "omnia".to_string(),
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
        let meta = sample_metadata();
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
            matches!(err, Error::Yaml(_) | Error::Config(_)),
            "expected Yaml/Config error, got {err:?}"
        );
    }

    #[test]
    fn deserializes_yaml_sample() {
        let yaml = r#"schema: omnia
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
            schema: "omnia".to_string(),
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
        // Walk both the post-RFC-13 layout (`.specify/slices/`) and the
        // pre-Phase-3 fixture layout (`.specify/changes/`) so this
        // smoke test keeps working through chunk 3.6 (the on-disk
        // migration).
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
    fn lifecycle_status_display_matches_serde() {
        assert_eq!(LifecycleStatus::Defining.to_string(), "defining");
        assert_eq!(LifecycleStatus::Defined.to_string(), "defined");
        assert_eq!(LifecycleStatus::Building.to_string(), "building");
        assert_eq!(LifecycleStatus::Complete.to_string(), "complete");
        assert_eq!(LifecycleStatus::Merged.to_string(), "merged");
        assert_eq!(LifecycleStatus::Dropped.to_string(), "dropped");
    }

    #[test]
    fn outcome_display_matches_serde() {
        assert_eq!(Outcome::Success.to_string(), "success");
        assert_eq!(Outcome::Failure.to_string(), "failure");
        assert_eq!(Outcome::Deferred.to_string(), "deferred");
        let proposal = Outcome::RegistryAmendmentRequired {
            proposed_name: "alpha-gateway".to_string(),
            proposed_url: "git@github.com:augentic/alpha-gateway.git".to_string(),
            proposed_schema: "omnia@v1".to_string(),
            proposed_description: None,
            rationale: "build discovered tangled code".to_string(),
        };
        assert_eq!(proposal.to_string(), "registry-amendment-required");
    }

    #[test]
    fn spec_type_display_matches_serde() {
        assert_eq!(SpecKind::New.to_string(), "new");
        assert_eq!(SpecKind::Modified.to_string(), "modified");
    }

    /// RFC-9 §2B back-compat invariant: the implicit pre-RFC-9
    /// metadata schema (no `version:` field, closed `Outcome`) must
    /// round-trip through the new reader. Resolved version is `1`.
    #[test]
    fn metadata_pre_rfc9_round_trips_with_default_version_one() {
        let yaml = r#"schema: omnia
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
        let meta: SliceMetadata = serde_saphyr::from_str(yaml).expect("parse pre-RFC-9 file");
        assert_eq!(meta.version, 1, "absent version should default to 1");
        assert_eq!(meta.status, LifecycleStatus::Complete);
        let stamped = meta.outcome.expect("outcome should round-trip");
        assert_eq!(stamped.phase, Phase::Merge);
        assert_eq!(stamped.outcome, Outcome::Success);
    }

    /// The new variant round-trips through serde (write → read →
    /// equal). This is the wire-format proof for the
    /// `registry-amendment-required` payload introduced by RFC-9 §2B.
    #[test]
    fn registry_amendment_required_round_trips_through_serde() {
        let stamp = Rfc3339Stamp::new("2024-08-04T11:22:33Z".to_string());
        let original = PhaseOutcome {
            phase: Phase::Build,
            outcome: Outcome::RegistryAmendmentRequired {
                proposed_name: "alpha-gateway".to_string(),
                proposed_url: "git@github.com:augentic/alpha-gateway.git".to_string(),
                proposed_schema: "omnia@v1".to_string(),
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

    /// A full `.metadata.yaml` carrying the new variant must
    /// round-trip end-to-end (load → save → load) — the writer
    /// stamps [`METADATA_VERSION`], the reader accepts it.
    #[test]
    fn metadata_with_registry_amendment_required_round_trips() {
        let dir = tempdir().expect("tempdir");
        let mut meta = sample_metadata();
        meta.outcome = Some(PhaseOutcome {
            phase: Phase::Build,
            outcome: Outcome::RegistryAmendmentRequired {
                proposed_name: "alpha-gateway".to_string(),
                proposed_url: "git@github.com:augentic/alpha-gateway.git".to_string(),
                proposed_schema: "omnia@v1".to_string(),
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
