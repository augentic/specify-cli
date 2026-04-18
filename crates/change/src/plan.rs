//! On-disk representation of `.specify/plan.yaml` and the in-memory
//! [`Plan`] state machine that wraps it.
//!
//! See `rfcs/rfc-2-plan.md` §"Library Implementation" for the canonical
//! type surface and `rfcs/rfc-2-plan.md` §"The Plan" for the reference
//! YAML fixture exercised by the round-trip tests.
//!
//! ## Scope of this file
//!
//! This Change (L1.A of the RFC-2 plan) only lands the *type surface*:
//! structs, enums, derives, and stubbed method signatures. Behaviour for
//! load/save, validation, transitions, topological ordering, and archival
//! is implemented in subsequent Changes (L1.B through L1.G). Every method
//! body below is a `todo!("Change L1.X — ...")` sentinel so later
//! subagents can `rg` for their assigned Change and fill in the bodies
//! without needing to move or re-shape any types.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Lifecycle state of a single entry in [`Plan::changes`].
///
/// The enum is `Copy + Eq + Hash` so it can appear in `HashSet`s,
/// `match` guards, and hash-keyed lookups without clones. This mirrors
/// the derives already used on `LifecycleStatus` in the parent module.
/// Transition-table methods (`can_transition_to`, `transition`) land in
/// Change L1.B and intentionally do not exist yet.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Done,
    Blocked,
    Failed,
    Skipped,
}

/// In-memory model of `.specify/plan.yaml`.
///
/// A `Plan` is an ordered, dependency-aware list of [`PlanChange`]s plus
/// a named map of [`Plan::sources`] (local paths or git URLs) that the
/// entries draw from.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Plan {
    /// Human-readable plan name, e.g. `platform-v2`.
    pub name: String,
    /// Named source locations referenced by [`PlanChange::sources`].
    /// Optional in the YAML; defaults to an empty map.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Ordered list of plan entries. Order is the *intended* execution
    /// order; the authoritative dependency-respecting order comes from
    /// [`Plan::topological_order`].
    pub changes: Vec<PlanChange>,
}

/// One entry in [`Plan::changes`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PlanChange {
    /// Stable identifier (kebab-case) unique within the plan.
    pub name: String,
    /// Current lifecycle state of this entry.
    pub status: PlanStatus,
    /// Names of other plan entries that must reach `done` before this
    /// entry is eligible.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Names of other plan entries this one logically *affects* (e.g.
    /// a bug-fix change whose scope modifies an already-done entry).
    #[serde(default)]
    pub affects: Vec<String>,
    /// Source keys (into [`Plan::sources`]) this entry draws from.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Free-form human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Operational explanation for the current non-terminal/terminal
    /// status (`failed`, `blocked`, or `skipped`). Overwritten on each
    /// status transition; cleared when the entry returns to `pending`,
    /// `in-progress`, or `done`. See §Fields.
    #[serde(default)]
    pub status_reason: Option<String>,
}

/// Patch applied by [`Plan::amend`] to an existing entry. Every field is
/// `Option<T>`; `None` means "leave unchanged", `Some(v)` means "replace
/// with v". `status` and `status_reason` are deliberately absent —
/// status transitions are made via [`Plan::transition`], never through
/// `amend`, and the reason field travels with the transition.
#[derive(Debug, Default, Clone)]
pub struct PlanChangePatch {
    /// Replace `depends_on` wholesale when `Some`.
    pub depends_on: Option<Vec<String>>,
    /// Replace `affects` wholesale when `Some`.
    pub affects: Option<Vec<String>>,
    /// Replace `sources` wholesale when `Some`.
    pub sources: Option<Vec<String>>,
    /// Replace `description` when `Some(Some(..))`; clear when
    /// `Some(None)`; leave unchanged when `None`.
    pub description: Option<Option<String>>,
}

/// Severity of a validation finding produced by [`Plan::validate`].
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationLevel {
    /// Blocking problem — the plan is not usable as-is.
    Error,
    /// Non-blocking advisory — the plan is usable but something looks
    /// off (e.g. a source key is defined but unreferenced).
    Warning,
}

/// A single finding reported by [`Plan::validate`].
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Severity bucket.
    pub level: ValidationLevel,
    /// Stable machine-readable code, e.g. `"plan.cycle"`.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
    /// Name of the offending entry, when the finding is entry-local.
    pub entry: Option<String>,
}

impl Plan {
    /// Load `.specify/plan.yaml` from disk.
    ///
    /// Errors mirror [`crate::ChangeMetadata::load`]:
    ///   - missing file -> `Error::Config`
    ///   - malformed YAML -> `Error::Yaml`
    ///   - other I/O failure -> `Error::Io`
    pub fn load(_path: &Path) -> Result<Self, Error> {
        todo!("Change L1.C — implement Plan::load")
    }

    /// Serialize and write the plan to `path`, overwriting if present.
    pub fn save(&self, _path: &Path) -> Result<(), Error> {
        todo!("Change L1.C — implement Plan::save")
    }

    /// Run all structural and semantic checks over the plan. The optional
    /// `changes_dir` points at `.specify/changes/` and enables the
    /// cross-reference checks against on-disk change metadata.
    pub fn validate(&self, _changes_dir: Option<&Path>) -> Vec<ValidationResult> {
        todo!("Change L1.D — implement Plan::validate")
    }

    /// First entry in topological order whose dependencies are all `done`
    /// and whose own status is `pending`. Returns `None` when nothing is
    /// eligible (plan finished, blocked, or empty).
    pub fn next_eligible(&self) -> Option<&PlanChange> {
        todo!("Change L1.E — implement Plan::next_eligible")
    }

    /// Transition the named entry to `target`, recording `reason` in
    /// [`PlanChange::status_reason`] per the rules documented in
    /// `rfc-2-plan.md` §Fields.
    pub fn transition(
        &mut self, _name: &str, _target: PlanStatus, _reason: Option<&str>,
    ) -> Result<(), Error> {
        todo!("Change L1.B — implement Plan::transition")
    }

    /// Append a new entry to the plan, rejecting duplicate names and
    /// unknown `depends_on` references.
    pub fn create(&mut self, _change: PlanChange) -> Result<(), Error> {
        todo!("Change L1.F — implement Plan::create")
    }

    /// Apply `patch` to the entry named `name`. `None` fields on the
    /// patch leave the corresponding `PlanChange` field unchanged.
    pub fn amend(&mut self, _name: &str, _patch: PlanChangePatch) -> Result<(), Error> {
        todo!("Change L1.F — implement Plan::amend")
    }

    /// Entries in dependency-respecting order. Errors with a cycle
    /// description when the `depends_on` graph contains a cycle.
    pub fn topological_order(&self) -> Result<Vec<&PlanChange>, Error> {
        todo!("Change L1.E — implement Plan::topological_order")
    }

    /// Move `.specify/plan.yaml` (and its companion state) into the
    /// archive directory. Refuses to archive plans with outstanding
    /// non-terminal entries unless `force` is set, in which case those
    /// entries are summarised in [`Error::PlanHasOutstandingWork`].
    pub fn archive(_path: &Path, _archive_dir: &Path, _force: bool) -> Result<PathBuf, Error> {
        todo!("Change L1.G — implement Plan::archive")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim reproduction of the `rfc-2-plan.md` §"The Plan" fixture.
    const RFC_EXAMPLE_YAML: &str = r#"name: platform-v2
sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git
changes:
  - name: user-registration
    sources: [monolith]
    status: done
  - name: email-verification
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress
  - name: registration-duplicate-email-crash
    affects: [user-registration]
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending
  - name: notification-preferences
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending
  - name: extract-shared-validation
    affects: [user-registration, email-verification]
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending
  - name: product-catalog
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending
  - name: shopping-cart
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending
  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
  - name: checkout-ui
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
"#;

    #[test]
    fn plan_roundtrips_rfc_example() {
        let original: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let rendered = serde_yaml::to_string(&original).expect("serialize plan");
        let reparsed: Plan = serde_yaml::from_str(&rendered).expect("reparse rendered plan");
        assert_eq!(original, reparsed, "plan should survive a serialize/parse round-trip");

        assert_eq!(original.name, "platform-v2");
        assert_eq!(original.sources.len(), 4);
        assert_eq!(original.changes.len(), 9);
        assert_eq!(original.changes[0].status, PlanStatus::Done);
        assert_eq!(original.changes[1].status, PlanStatus::InProgress);
        assert_eq!(original.changes[7].status, PlanStatus::Failed);
        assert!(original.changes[7].status_reason.is_some());
    }

    #[test]
    fn kebab_case_serialization() {
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "entry-one".to_string(),
                status: PlanStatus::InProgress,
                depends_on: vec!["entry-zero".to_string()],
                affects: vec![],
                sources: vec![],
                description: None,
                status_reason: Some("awaiting upstream fix".to_string()),
            }],
        };
        let yaml = serde_yaml::to_string(&plan).expect("serialize plan");
        assert!(yaml.contains("depends-on:"), "expected kebab-case depends-on in:\n{yaml}");
        assert!(
            yaml.contains("status: in-progress"),
            "expected kebab-case enum value in-progress in:\n{yaml}"
        );
        assert!(yaml.contains("status-reason:"), "expected kebab-case status-reason in:\n{yaml}");
        assert!(!yaml.contains("depends_on"), "snake_case depends_on leaked into output:\n{yaml}");
        assert!(
            !yaml.contains("status_reason"),
            "snake_case status_reason leaked into output:\n{yaml}"
        );
    }

    #[test]
    fn missing_optional_fields_deserialize_with_defaults() {
        let yaml = "name: foo\nchanges: []\n";
        let plan: Plan = serde_yaml::from_str(yaml).expect("parse minimal plan");
        assert_eq!(plan.name, "foo");
        assert!(plan.sources.is_empty(), "sources should default to empty map");
        assert!(plan.changes.is_empty(), "changes should be empty");
    }

    #[test]
    fn status_reason_roundtrips_on_failed_entry() {
        let yaml = r#"name: demo
changes:
  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
"#;
        let plan: Plan = serde_yaml::from_str(yaml).expect("parse");
        let entry = &plan.changes[0];
        assert_eq!(entry.status, PlanStatus::Failed);
        let reason = entry.status_reason.as_deref().expect("status_reason populated");
        assert!(
            reason.contains("Type mismatch"),
            "status_reason should preserve folded text, got: {reason:?}"
        );

        let rendered = serde_yaml::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_yaml::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed);
        assert_eq!(
            reparsed.changes[0].status_reason, entry.status_reason,
            "status_reason should be byte-identical after round-trip"
        );
    }
}
