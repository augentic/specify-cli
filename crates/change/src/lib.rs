//! `.metadata.yaml` lifecycle state machine for Specify changes.
//!
//! Exposes the on-disk `ChangeMetadata` document that lives at
//! `<change_dir>/.metadata.yaml`, plus the `LifecycleStatus` state machine
//! that gates legal transitions between `Defining`, `Defined`, `Building`,
//! `Complete`, `Merged`, and `Dropped`.
//!
//! See `rfcs/rfc-1-cli.md` §`metadata.rs` for the canonical transition
//! graph and the `rfcs/rfc-1-plan.md` §"Change F" for scope.
//!
//! The [`actions`] submodule layers the verb-level operations
//! (`create`, `transition`, `archive`, `drop`, `scan_touched_specs`,
//! `overlap`) that the `specify change` subcommand dispatches to.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;
pub use specify_schema::Phase;

/// Verb-level operations on a Specify change directory.
pub mod actions;
mod atomic;
/// On-disk journal for append-only audit logging.
pub mod journal;
/// Advisory PID lock for the Layer 2 executor.
pub mod lock;
/// Plan state machine for ordered, dependency-aware change execution.
pub mod plan;
/// `specify plan doctor` — RFC-9 §4B plan-health diagnostics.
pub mod plan_doctor;
/// RFC 3339 timestamp newtype.
pub mod timestamp;

pub use actions::{CreateIfExists, CreateOutcome, Overlap, format_rfc3339, is_valid_kebab_name};
pub use journal::{EntryKind, Journal, JournalEntry};
pub use lock::{Acquired, Guard, PlanLockReleased, PlanLockState, Stamp};
pub use plan::{Entry, EntryPatch, Finding, Plan, Severity, Status};
pub use plan_doctor::{
    BlockingPredecessor, CODE_CYCLE, CODE_ORPHAN_SOURCE, CODE_STALE_CLONE, CODE_UNREACHABLE,
    CloneSignature, Diagnostic as PlanDoctorDiagnostic, DiagnosticPayload as PlanDoctorPayload,
    DiagnosticSeverity as PlanDoctorSeverity, StaleCloneReason, doctor as plan_doctor,
};
pub use timestamp::Rfc3339Stamp;

/// On-disk schema version for `.metadata.yaml`.
///
/// Bumped by RFC-9 §2B (`rfc9-2b-plan-registry-proposal`) from the
/// implicit pre-RFC-9 version `1` to `2` when the
/// [`Outcome::RegistryAmendmentRequired`] variant landed. The version is
/// **informational** for the reader: the on-disk `outcome` field is
/// dispatched purely on its serde discriminant (`success` /
/// `failure` / `deferred` / `registry-amendment-required`), so a v1
/// file with one of the original three outcomes round-trips cleanly
/// through a v2 reader without any version-specific branching. New
/// writes always emit the current version; the constant therefore
/// pins the value the **writer** stamps, not a gate the **reader**
/// enforces.
///
/// Pre-RFC-9 archived metadata files have no `version` field at all;
/// `serde(default)` resolves them to `1` on read (see
/// `default_metadata_version`).
pub const METADATA_VERSION: u32 = 2;

/// Default value for [`ChangeMetadata::version`] when the field is
/// absent on disk. Pre-RFC-9 archives lack the field entirely; serde's
/// `default` shim resolves them to the implicit version `1`.
const fn default_metadata_version() -> u32 {
    1
}

/// On-disk representation of `<change_dir>/.metadata.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ChangeMetadata {
    /// On-disk schema version. Pre-RFC-9 archives lack this field;
    /// `serde(default)` resolves them to `1`. Current writers always
    /// emit [`METADATA_VERSION`] (`2`). The reader does not gate on
    /// the value — it is informational, used by tooling that wants
    /// to surface "this is an old archive" diagnostics. Outcome
    /// dispatch happens purely on the serde discriminant on the
    /// `outcome.outcome` field.
    #[serde(default = "default_metadata_version")]
    pub version: u32,
    /// Schema identifier for this change (e.g. `omnia`).
    pub schema: String,
    /// Current lifecycle state.
    pub status: LifecycleStatus,
    /// When the change was created.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_at: Option<Rfc3339Stamp>,
    /// When the change entered `Defined`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub defined_at: Option<Rfc3339Stamp>,
    /// When the build phase started.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub build_started_at: Option<Rfc3339Stamp>,
    /// When the change reached `Complete`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completed_at: Option<Rfc3339Stamp>,
    /// When the change was merged.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub merged_at: Option<Rfc3339Stamp>,
    /// When the change was dropped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dropped_at: Option<Rfc3339Stamp>,
    /// Human-readable reason for dropping the change.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub drop_reason: Option<String>,
    /// Specs affected by this change.
    #[serde(default)]
    pub touched_specs: Vec<TouchedSpec>,
    /// Outcome of the most recent phase run.
    ///
    /// Writer contract: this field is written by two code paths:
    ///
    /// 1. `specify change phase-outcome` (see [`actions::phase_outcome`]) —
    ///    used by phase skills for `failure` and `deferred` outcomes
    ///    (and by `define`/`build` for `success`).
    /// 2. `specify_merge::merge_change` — stamps `Success` atomically
    ///    with the `Merged` status transition, before the archive move.
    ///    This is necessary because the archive move removes the change
    ///    from `.specify/changes/`, making a subsequent `phase-outcome`
    ///    call impossible.
    ///
    /// Phase skills (`define`, `build`, `merge`) must never edit
    /// `.metadata.yaml` directly — they go through the CLI so the write
    /// is atomic and the single-field overwrite semantics (latest
    /// outcome only — no history) are preserved. Consumers:
    /// `/spec:execute` reads this on phase return to decide the
    /// next plan transition per RFC-2 §"Phase Outcome Contract".
    ///
    /// Stored as a single `Option<PhaseOutcome>` (not a list): a new
    /// stamp overwrites the previous outcome. Journal/history lives
    /// elsewhere (`journal.yaml`, L2.B).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PhaseOutcome>,
}

/// Result of a phase run (define | build | merge) as recorded in
/// `.metadata.yaml`. Read by `/spec:execute` on phase return to decide
/// the next plan transition (see RFC-2 §"Phase Outcome Contract").
///
/// Written by `specify change phase-outcome` (for define/build phases
/// and merge failure/deferred) and by `merge_change` (for merge
/// success, stamped atomically before the archive move).
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

/// The classifications a phase returns to `/spec:execute`.
///
/// The first three variants are unit ones; their on-disk shape is the
/// bare kebab-case discriminant (`outcome: success`). The fourth
/// variant — added by RFC-9 §2B (`rfc9-2b-plan-registry-proposal`) —
/// carries a structured payload describing a registry amendment the
/// phase wants the operator to apply before the change can land. Its
/// on-disk shape is therefore an externally-tagged map rather than a
/// bare string:
///
/// ```yaml
/// outcome:
///   registry-amendment-required:
///     proposed-name: alpha-gateway
///     proposed-url: git@github.com:augentic/alpha-gateway.git
///     proposed-schema: omnia@v1
///     proposed-description: "Gateway service for alpha capability."
///     rationale: "Build discovered tangled code requiring split."
/// ```
///
/// **Wire-format compatibility.** Because serde's external tag
/// representation deserialises unit and struct variants by the same
/// discriminant rules, a pre-RFC-9 file with `outcome: success`
/// continues to round-trip through this enum unchanged. Conversely,
/// older binaries (without the new variant) error cleanly on
/// `registry-amendment-required` payloads — they cannot silently mis-
/// route them as one of the original three. The
/// [`crate::METADATA_VERSION`] constant is the stamp writers attach so
/// tooling can surface "this archive predates the new variant"
/// diagnostics.
///
/// Implements `Clone + PartialEq + Eq` (no `Copy` because the new
/// variant carries `String` fields).
///
/// `rename_all = "kebab-case"` discriminantises the variants;
/// `rename_all_fields = "kebab-case"` does the same for the struct
/// variant's fields so the on-disk shape stays kebab-case end-to-end.
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
    /// Phase blocked on a registry amendment the operator must apply
    /// before the change can land. Recorded by phase skills via
    /// `specify change outcome set <change> <phase> registry-amendment-required ...`.
    /// `/spec:execute` treats this exactly like `deferred` — it drops
    /// the change and transitions the plan entry to `blocked` — and
    /// surfaces the proposal payload via the change's journal so the
    /// operator can run the canonical recovery sequence (see
    /// `plugins/spec/skills/execute/SKILL.md` →
    /// §"Registry amendment required (RFC-9 §2B)").
    RegistryAmendmentRequired {
        /// Kebab-case project name proposed for the registry.
        proposed_name: String,
        /// Clone URL for the proposed project (git remote, ssh, http,
        /// or `git+...://`). Same shape rules as
        /// `specify registry add --url`.
        proposed_url: String,
        /// Schema identifier the proposed project should adopt
        /// (e.g. `omnia@v1`).
        proposed_schema: String,
        /// Optional human-readable description of the proposed
        /// project (RFC-3b `description-missing-multi-repo`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proposed_description: Option<String>,
        /// Free-form prose explaining why the phase decided this
        /// amendment was required. Surfaced verbatim to the operator.
        rationale: String,
    },
}

/// Lifecycle states a change passes through.
///
/// `Copy + Eq + Hash` are additive to RFC-1 so the enum can participate in
/// `HashSet`s (used by the exhaustive transition property test) and match
/// guards without requiring explicit clones.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LifecycleStatus {
    /// Change is being defined (artifacts authored).
    Defining,
    /// Definition complete, awaiting build.
    Defined,
    /// Build phase in progress.
    Building,
    /// Build complete, awaiting merge.
    Complete,
    /// Specs merged into baseline and change archived.
    Merged,
    /// Change discarded without merging.
    Dropped,
}

/// One entry in `ChangeMetadata::touched_specs`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct TouchedSpec {
    /// Capability name (kebab-case).
    pub name: String,
    /// Whether this spec is new or modifies an existing baseline.
    #[serde(rename = "type")]
    pub kind: SpecType,
}

/// Whether a touched spec is new or a modification of an existing baseline.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpecType {
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
    /// Render the kebab-case discriminant only — payload fields are
    /// not formatted. Callers that need the proposal payload (the
    /// only variant that carries one) round-trip through serde and
    /// emit the structured shape directly. Mirrors the on-disk
    /// discriminant string so `outcome.to_string()` keeps matching
    /// `outcome:` in YAML.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.discriminant())
    }
}

impl Outcome {
    /// Kebab-case discriminant string for this outcome.
    ///
    /// The discriminant matches the on-disk serde tag — the bare
    /// string used for unit variants and the map key used for the
    /// struct variant. Stable: tooling that branches on this string
    /// (e.g. `/spec:execute` classifying `registry-amendment-required`
    /// as `blocked`) reads from this contract.
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

impl fmt::Display for SpecType {
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
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Merged | Self::Dropped)
    }

    /// Whether `self → target` is a legal edge in the lifecycle state machine.
    #[must_use]
    pub const fn can_transition_to(&self, target: &Self) -> bool {
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
    pub fn transition(&self, target: Self) -> Result<Self, Error> {
        if self.can_transition_to(&target) {
            Ok(target)
        } else {
            Err(Error::Lifecycle {
                expected: format!("valid transition from {self:?}"),
                found: format!("{target:?}"),
            })
        }
    }
}

impl ChangeMetadata {
    /// Convenience helper: `<change_dir>/.metadata.yaml`.
    #[must_use]
    pub fn path(change_dir: &Path) -> PathBuf {
        change_dir.join(".metadata.yaml")
    }

    /// Load `.metadata.yaml` from a change directory.
    ///
    /// Errors:
    ///   - file missing -> `Error::Config` with path context
    ///   - YAML malformed -> `Error::Yaml` (via `From<serde_saphyr::Error>`)
    ///   - other I/O failure -> `Error::Io`
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(change_dir: &Path) -> Result<Self, Error> {
        let path = Self::path(change_dir);
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

    /// Write `.metadata.yaml` to a change directory. Overwrites if present.
    ///
    /// Atomic: a partial file is never observed by readers. Write goes
    /// via a temp file in the same directory followed by `fs::rename`.
    /// This mirrors the exact convention used by [`Plan::save`] — both
    /// `ChangeMetadata` and `Plan` route their on-disk writes through
    /// `NamedTempFile::new_in(parent) + persist` so that every
    /// `.specify/*.yaml` write in the codebase is crash-safe and
    /// never-partial under concurrent reads.
    ///
    /// A trailing newline is always emitted so the on-disk form
    /// matches the convention used by `Plan::save` and so POSIX
    /// text-file tools (`wc -l`, `sed`, `grep`) behave predictably.
    ///
    /// Does **not** create the parent directory — `init`/`define` own
    /// that responsibility. Returns `Error::Io` on any write failure
    /// and `Error::Yaml` if serialization fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn save(&self, change_dir: &Path) -> Result<(), Error> {
        let path = Self::path(change_dir);
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
                let actual = from.can_transition_to(&to);
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
                    !from.can_transition_to(&to),
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

    fn sample_metadata() -> ChangeMetadata {
        ChangeMetadata {
            version: METADATA_VERSION,
            schema: "omnia".to_string(),
            status: LifecycleStatus::Building,
            created_at: Some(Rfc3339Stamp::from_raw("2024-08-01T10:00:00Z".to_string())),
            defined_at: Some(Rfc3339Stamp::from_raw("2024-08-01T12:00:00Z".to_string())),
            build_started_at: Some(Rfc3339Stamp::from_raw("2024-08-02T09:30:00Z".to_string())),
            completed_at: Some(Rfc3339Stamp::from_raw("2024-08-03T15:45:00Z".to_string())),
            merged_at: None,
            dropped_at: None,
            drop_reason: None,
            touched_specs: vec![
                TouchedSpec {
                    name: "login".to_string(),
                    kind: SpecType::Modified,
                },
                TouchedSpec {
                    name: "oauth".to_string(),
                    kind: SpecType::New,
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
        let loaded = ChangeMetadata::load(dir.path()).expect("load ok");
        assert_eq!(loaded, meta);
    }

    #[test]
    fn load_missing_returns_not_found() {
        let dir = tempdir().expect("tempdir");
        let err = ChangeMetadata::load(dir.path()).expect_err("expected error");
        match err {
            Error::ArtifactNotFound { kind, path } => {
                assert_eq!(kind, ".metadata.yaml");
                assert!(
                    path.display().to_string().contains(&dir.path().display().to_string()),
                    "path should include change dir, got: {}",
                    path.display()
                );
            }
            other => panic!("expected Error::ArtifactNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_malformed_yaml_errors() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(ChangeMetadata::path(dir.path()), "not: a\n  valid: yaml\n: structure:")
            .expect("write ok");
        let err = ChangeMetadata::load(dir.path()).expect_err("expected error");
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
        let meta: ChangeMetadata = serde_saphyr::from_str(yaml).expect("parse ok");
        assert_eq!(meta.status, LifecycleStatus::Building);
        assert_eq!(meta.created_at.as_deref(), Some("2024-08-01T10:00:00Z"));
        assert_eq!(meta.defined_at.as_deref(), Some("2024-08-01T12:00:00Z"));
        assert_eq!(meta.build_started_at.as_deref(), Some("2024-08-02T09:30:00Z"));
        assert_eq!(meta.completed_at, None);
        assert_eq!(meta.touched_specs.len(), 2);
        assert_eq!(meta.touched_specs[0].name, "login");
        assert_eq!(meta.touched_specs[0].kind, SpecType::Modified);
        assert_eq!(meta.touched_specs[1].name, "oauth");
        assert_eq!(meta.touched_specs[1].kind, SpecType::New);
    }

    #[test]
    fn serializes_kebab_case_and_lowercase_enums() {
        let meta = ChangeMetadata {
            version: METADATA_VERSION,
            schema: "omnia".to_string(),
            status: LifecycleStatus::Building,
            created_at: Some(Rfc3339Stamp::from_raw("2024-08-01T10:00:00Z".to_string())),
            defined_at: None,
            build_started_at: Some(Rfc3339Stamp::from_raw("2024-08-02T09:30:00Z".to_string())),
            completed_at: None,
            merged_at: None,
            dropped_at: None,
            drop_reason: None,
            touched_specs: vec![TouchedSpec {
                name: "login".to_string(),
                kind: SpecType::Modified,
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
    fn real_world_change_file_parses() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let changes_dir = manifest
            .parent()
            .and_then(|p| p.parent())
            .map(|repo_root| repo_root.join(".specify").join("changes"));
        let Some(changes_dir) = changes_dir else {
            return;
        };
        let Ok(read_dir) = std::fs::read_dir(&changes_dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let change_path = entry.path();
            if !change_path.is_dir() {
                continue;
            }
            if !ChangeMetadata::path(&change_path).exists() {
                continue;
            }
            ChangeMetadata::load(&change_path).unwrap_or_else(|e| {
                panic!("existing metadata at {} should parse, got {e:?}", change_path.display())
            });
            return;
        }
    }

    #[test]
    fn path_appends_metadata_yaml() {
        let dir = Path::new("/tmp/some/change");
        assert_eq!(ChangeMetadata::path(dir), PathBuf::from("/tmp/some/change/.metadata.yaml"));
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
        assert_eq!(SpecType::New.to_string(), "new");
        assert_eq!(SpecType::Modified.to_string(), "modified");
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
        let meta: ChangeMetadata = serde_saphyr::from_str(yaml).expect("parse pre-RFC-9 file");
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
        let stamp = Rfc3339Stamp::from_raw("2024-08-04T11:22:33Z".to_string());
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
            at: Rfc3339Stamp::from_raw("2024-08-04T11:22:33Z".to_string()),
            summary: "registry-amendment-required: alpha-gateway".to_string(),
            context: None,
        });
        meta.save(dir.path()).expect("save ok");
        let loaded = ChangeMetadata::load(dir.path()).expect("load ok");
        assert_eq!(loaded.version, METADATA_VERSION);
        assert_eq!(loaded, meta);
        let on_disk =
            std::fs::read_to_string(ChangeMetadata::path(dir.path())).expect("read raw file");
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
