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

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;
pub use specify_schema::Phase;

pub mod actions;
pub mod plan;

pub use actions::{CreateIfExists, CreateOutcome, Overlap};
pub use plan::*;

/// On-disk representation of `<change_dir>/.metadata.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct ChangeMetadata {
    pub schema: String,
    pub status: LifecycleStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub defined_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub build_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub merged_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dropped_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub drop_reason: Option<String>,
    #[serde(default)]
    pub touched_specs: Vec<TouchedSpec>,
    /// Outcome of the most recent phase run recorded by
    /// `specify change phase-outcome`.
    ///
    /// Writer contract: this field is written **only** by the
    /// `specify change phase-outcome` CLI subcommand (see
    /// [`actions::phase_outcome`]). Phase skills (`define`, `build`,
    /// `merge`) must never edit `.metadata.yaml` directly — they go
    /// through the CLI so the write is atomic and the single-field
    /// overwrite semantics (latest outcome only — no history) are
    /// preserved. Consumers:
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
/// Written by the `specify change phase-outcome` subcommand; phases
/// never edit `.metadata.yaml` directly.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct PhaseOutcome {
    pub phase: Phase,
    pub outcome: Outcome,
    pub at: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// The three possible outcomes a phase returns to `/spec:execute`.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Success,
    Failure,
    Deferred,
}

/// Lifecycle states a change passes through.
///
/// `Copy + Eq + Hash` are additive to RFC-1 so the enum can participate in
/// `HashSet`s (used by the exhaustive transition property test) and match
/// guards without requiring explicit clones.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleStatus {
    Defining,
    Defined,
    Building,
    Complete,
    Merged,
    Dropped,
}

/// One entry in `ChangeMetadata::touched_specs`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct TouchedSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub spec_type: SpecType,
}

/// Whether a touched spec is new or a modification of an existing baseline.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpecType {
    New,
    Modified,
}

impl LifecycleStatus {
    /// The creation edge (`START → Defining`). Called by `init`/`define`.
    pub fn initial() -> Self {
        LifecycleStatus::Defining
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, LifecycleStatus::Merged | LifecycleStatus::Dropped)
    }

    pub fn can_transition_to(&self, target: &Self) -> bool {
        use LifecycleStatus::*;
        matches!(
            (self, target),
            (Defining, Defined)
                | (Defined, Defining)
                | (Defined, Building)
                | (Building, Complete)
                | (Complete, Merged)
                | (Defining, Complete)
                | (Defining | Defined | Building | Complete, Dropped)
        )
    }

    pub fn transition(&self, target: LifecycleStatus) -> Result<LifecycleStatus, Error> {
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
    pub fn path(change_dir: &Path) -> PathBuf {
        change_dir.join(".metadata.yaml")
    }

    /// Load `.metadata.yaml` from a change directory.
    ///
    /// Errors:
    ///   - file missing -> `Error::Config` with path context
    ///   - YAML malformed -> `Error::Yaml` (via `From<serde_yaml::Error>`)
    ///   - other I/O failure -> `Error::Io`
    pub fn load(change_dir: &Path) -> Result<Self, Error> {
        let path = Self::path(change_dir);
        if !path.exists() {
            return Err(Error::Config(format!(
                ".metadata.yaml not found in {}",
                change_dir.display()
            )));
        }
        let content = std::fs::read_to_string(&path)?;
        let meta: ChangeMetadata = serde_yaml::from_str(&content)?;
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
    pub fn save(&self, change_dir: &Path) -> Result<(), Error> {
        let path = Self::path(change_dir);
        let mut content = serde_yaml::to_string(self)?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(tmp.as_file_mut(), content.as_bytes())?;
        tmp.as_file_mut().sync_all()?;
        tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::tempdir;

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
    fn exhaustive_transition_table_matches_allowed_set() {
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
    fn terminal_states_have_no_outgoing_edges() {
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
    fn every_legal_edge_round_trips_through_transition() {
        for (from, to) in allowed_edges() {
            let result = from
                .transition(to)
                .unwrap_or_else(|e| panic!("expected {from:?} -> {to:?} to succeed, got {e:?}"));
            assert_eq!(result, to);
        }
    }

    #[test]
    fn every_illegal_edge_returns_lifecycle_error() {
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
            schema: "omnia".to_string(),
            status: LifecycleStatus::Building,
            created_at: Some("2024-08-01T10:00:00Z".to_string()),
            defined_at: Some("2024-08-01T12:00:00Z".to_string()),
            build_started_at: Some("2024-08-02T09:30:00Z".to_string()),
            completed_at: Some("2024-08-03T15:45:00Z".to_string()),
            merged_at: None,
            dropped_at: None,
            drop_reason: None,
            touched_specs: vec![
                TouchedSpec {
                    name: "login".to_string(),
                    spec_type: SpecType::Modified,
                },
                TouchedSpec {
                    name: "oauth".to_string(),
                    spec_type: SpecType::New,
                },
            ],
            outcome: None,
        }
    }

    #[test]
    fn save_then_load_round_trips_all_fields() {
        let dir = tempdir().expect("tempdir");
        let meta = sample_metadata();
        meta.save(dir.path()).expect("save ok");
        let loaded = ChangeMetadata::load(dir.path()).expect("load ok");
        assert_eq!(loaded, meta);
    }

    #[test]
    fn load_missing_file_returns_config_error() {
        let dir = tempdir().expect("tempdir");
        let err = ChangeMetadata::load(dir.path()).expect_err("expected error");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains(".metadata.yaml not found"), "unexpected message: {msg}");
                assert!(
                    msg.contains(&dir.path().display().to_string()),
                    "message should include change dir path, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn load_malformed_yaml_returns_err() {
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
    fn deserializes_representative_yaml_sample() {
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
        let meta: ChangeMetadata = serde_yaml::from_str(yaml).expect("parse ok");
        assert_eq!(meta.status, LifecycleStatus::Building);
        assert_eq!(meta.created_at.as_deref(), Some("2024-08-01T10:00:00Z"));
        assert_eq!(meta.defined_at.as_deref(), Some("2024-08-01T12:00:00Z"));
        assert_eq!(meta.build_started_at.as_deref(), Some("2024-08-02T09:30:00Z"));
        assert_eq!(meta.completed_at, None);
        assert_eq!(meta.touched_specs.len(), 2);
        assert_eq!(meta.touched_specs[0].name, "login");
        assert_eq!(meta.touched_specs[0].spec_type, SpecType::Modified);
        assert_eq!(meta.touched_specs[1].name, "oauth");
        assert_eq!(meta.touched_specs[1].spec_type, SpecType::New);
    }

    #[test]
    fn serializes_kebab_case_fields_and_lowercase_enums() {
        let meta = ChangeMetadata {
            schema: "omnia".to_string(),
            status: LifecycleStatus::Building,
            created_at: Some("2024-08-01T10:00:00Z".to_string()),
            defined_at: None,
            build_started_at: Some("2024-08-02T09:30:00Z".to_string()),
            completed_at: None,
            merged_at: None,
            dropped_at: None,
            drop_reason: None,
            touched_specs: vec![TouchedSpec {
                name: "login".to_string(),
                spec_type: SpecType::Modified,
            }],
            outcome: None,
        };
        let yaml = serde_yaml::to_string(&meta).expect("serialize ok");
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
    fn real_world_change_file_is_parseable_if_present() {
        // Look for `<repo>/.specify/changes/<something>/.metadata.yaml`.
        // `CARGO_MANIFEST_DIR` points at `<repo>/crates/change` at test time.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let changes_dir = manifest
            .parent()
            .and_then(|p| p.parent())
            .map(|repo_root| repo_root.join(".specify").join("changes"));
        let Some(changes_dir) = changes_dir else {
            return;
        };
        let Ok(read_dir) = std::fs::read_dir(&changes_dir) else {
            return; // fresh clone — no changes to parse
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
    fn path_helper_appends_metadata_yaml() {
        let dir = Path::new("/tmp/some/change");
        assert_eq!(ChangeMetadata::path(dir), PathBuf::from("/tmp/some/change/.metadata.yaml"));
    }
}
