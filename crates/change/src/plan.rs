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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::DiGraph;
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

impl PlanStatus {
    /// Every variant in declaration order. Used by exhaustive transition
    /// tests here and by validation/topological code in L1.D/E that
    /// needs to enumerate states without depending on `strum`.
    pub const ALL: [PlanStatus; 6] = [
        PlanStatus::Pending,
        PlanStatus::InProgress,
        PlanStatus::Done,
        PlanStatus::Blocked,
        PlanStatus::Failed,
        PlanStatus::Skipped,
    ];

    /// Whether `self -> target` is a legal edge in the plan-entry state
    /// machine. See `rfc-2-plan.md` §"Transition Rules" for the canonical
    /// table; the 10 edges enumerated below are the *only* legal ones.
    /// `Done` is terminal: every edge with `Done` on the left is `false`.
    pub fn can_transition_to(&self, target: &PlanStatus) -> bool {
        use PlanStatus::*;
        matches!(
            (self, target),
            (Pending, InProgress)
                | (Pending, Blocked)
                | (Pending, Skipped)
                | (InProgress, Done)
                | (InProgress, Failed)
                | (InProgress, Blocked)
                | (Blocked, Pending)
                | (Failed, Pending)
                | (Failed, Skipped)
                | (Skipped, Pending)
        )
    }

    /// Return `target` if the edge is legal, otherwise an
    /// `Error::PlanTransition` carrying both endpoints by their `Debug`
    /// representation. Mirrors `LifecycleStatus::transition`.
    pub fn transition(&self, target: PlanStatus) -> Result<PlanStatus, Error> {
        if self.can_transition_to(&target) {
            Ok(target)
        } else {
            Err(Error::PlanTransition {
                from: format!("{self:?}"),
                to: format!("{target:?}"),
            })
        }
    }
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
    ///
    /// Tolerant of files with or without a trailing newline —
    /// `serde_yaml::from_str` accepts both.
    pub fn load(path: &Path) -> Result<Self, Error> {
        if !path.exists() {
            return Err(Error::Config(format!("plan.yaml not found at {}", path.display())));
        }
        let content = std::fs::read_to_string(path)?;
        let plan: Plan = serde_yaml::from_str(&content)?;
        Ok(plan)
    }

    /// Serialize and write the plan to `path`, overwriting if present.
    ///
    /// Atomic: a partial file is never observed by readers. Write goes via
    /// a temp file in the same directory followed by `fs::rename`. Because
    /// POSIX `rename(2)` (and Windows `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`)
    /// are atomic at the filesystem level, any concurrent reader of `path`
    /// sees either the previous complete contents or the new complete
    /// contents — never a half-written or empty file. Placing the temp
    /// file in `path.parent()` keeps the rename on the same filesystem,
    /// which is the precondition that makes the rename atomic rather than
    /// a copy-then-unlink.
    ///
    /// Always emits a trailing newline so the on-disk form matches the
    /// convention used elsewhere in the project and so POSIX text-file
    /// tools (`wc -l`, `sed`, `grep`) behave predictably.
    ///
    /// Returns `Error::Io` on any I/O failure and `Error::Yaml` if
    /// serialization fails.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let mut content = serde_yaml::to_string(self)?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(tmp.as_file_mut(), content.as_bytes())?;
        tmp.as_file_mut().sync_all()?;
        tmp.persist(path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// Run all structural and semantic checks over the plan. The optional
    /// `changes_dir` points at `.specify/changes/` and enables the
    /// cross-reference checks against on-disk change metadata.
    ///
    /// Findings are accumulated — no check short-circuits another. Order
    /// is structural checks first (duplicate names, cycles, unknown
    /// depends-on / affects / sources, multiple in-progress) followed by
    /// consistency checks against `changes_dir` when provided.
    ///
    /// Note on "well-formed status values": `PlanStatus` is an enum, so
    /// every in-memory instance is well-formed by construction. serde
    /// rejects invalid statuses at parse time. The RFC lists this check
    /// for completeness against hand-edited YAML that bypassed parsing,
    /// which is not reachable in-process — so nothing is emitted for it.
    pub fn validate(&self, changes_dir: Option<&Path>) -> Vec<ValidationResult> {
        let mut results = Vec::new();
        results.extend(collect_duplicate_names(&self.changes));
        results.extend(detect_cycles(&self.changes));
        results.extend(check_unknown_depends_on(&self.changes));
        results.extend(check_unknown_affects(&self.changes));
        results.extend(check_unknown_sources(self));
        results.extend(check_single_in_progress(&self.changes));
        if let Some(dir) = changes_dir.filter(|d| d.is_dir()) {
            results.extend(check_changes_dir_consistency(self, dir));
        }
        results
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

/// Emit one `duplicate-name` error per duplicate *occurrence* (every
/// occurrence after the first).
fn collect_duplicate_names(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for entry in changes {
        if !seen.insert(entry.name.as_str()) {
            out.push(ValidationResult {
                level: ValidationLevel::Error,
                code: "duplicate-name",
                message: format!("duplicate plan entry name '{}'", entry.name),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

/// Build a `depends_on -> self` DAG and emit one `dependency-cycle`
/// result per cycle (including self-edges). Uses `petgraph::toposort`
/// to detect the existence of a cycle, then `tarjan_scc` to enumerate
/// every strongly-connected component larger than one node plus any
/// self-edges (which are their own SCC of size 1 with a loop).
fn detect_cycles(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let mut graph: DiGraph<&str, ()> = DiGraph::new();
    let mut idx = HashMap::new();
    for entry in changes {
        let node = graph.add_node(entry.name.as_str());
        idx.insert(entry.name.as_str(), node);
    }
    let mut has_self_loop = false;
    for entry in changes {
        let to = idx[entry.name.as_str()];
        for dep in &entry.depends_on {
            if let Some(&from) = idx.get(dep.as_str()) {
                graph.add_edge(from, to, ());
                if from == to {
                    has_self_loop = true;
                }
            }
        }
    }

    if toposort(&graph, None).is_ok() && !has_self_loop {
        return Vec::new();
    }

    let mut out = Vec::new();
    for scc in tarjan_scc(&graph) {
        if scc.len() > 1 {
            let mut names: Vec<&str> = scc.iter().map(|&n| graph[n]).collect();
            names.sort_unstable();
            let mut path = names.clone();
            path.push(names[0]);
            out.push(ValidationResult {
                level: ValidationLevel::Error,
                code: "dependency-cycle",
                message: format!("cycle: {}", path.join(" → ")),
                entry: None,
            });
        } else if scc.len() == 1 {
            let node = scc[0];
            if graph.find_edge(node, node).is_some() {
                let name = graph[node];
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
                    code: "dependency-cycle",
                    message: format!("cycle: {name} → {name}"),
                    entry: None,
                });
            }
        }
    }
    out
}

/// Emit one `unknown-depends-on` error per missing target.
fn check_unknown_depends_on(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.depends_on {
            if !known.contains(target.as_str()) {
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
                    code: "unknown-depends-on",
                    message: format!("depends-on references unknown change '{target}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Emit one `unknown-affects` error per missing target.
fn check_unknown_affects(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.affects {
            if !known.contains(target.as_str()) {
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
                    code: "unknown-affects",
                    message: format!("affects references unknown change '{target}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Emit one `unknown-source` error per source key not declared at the
/// plan level.
fn check_unknown_sources(plan: &Plan) -> Vec<ValidationResult> {
    let mut out = Vec::new();
    for entry in &plan.changes {
        for key in &entry.sources {
            if !plan.sources.contains_key(key) {
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
                    code: "unknown-source",
                    message: format!("sources references unknown source key '{key}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// When more than one entry is `in-progress`, emit one result per
/// offending entry so every offender is surfaceable in the UI.
fn check_single_in_progress(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let offenders: Vec<&PlanChange> =
        changes.iter().filter(|c| c.status == PlanStatus::InProgress).collect();
    if offenders.len() <= 1 {
        return Vec::new();
    }
    offenders
        .into_iter()
        .map(|c| ValidationResult {
            level: ValidationLevel::Error,
            code: "multiple-in-progress",
            message: "multiple in-progress entries: at most one allowed per plan".to_string(),
            entry: Some(c.name.clone()),
        })
        .collect()
}

/// Plan-to-change directory consistency:
///   - Warn on orphan subdirectories (no matching plan entry).
///   - Warn when an `in-progress` plan entry has no matching directory.
fn check_changes_dir_consistency(plan: &Plan, changes_dir: &Path) -> Vec<ValidationResult> {
    let mut out = Vec::new();
    let declared: HashSet<&str> = plan.changes.iter().map(|c| c.name.as_str()).collect();

    let Ok(read_dir) = std::fs::read_dir(changes_dir) else {
        return out;
    };
    let mut dir_names: Vec<String> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        dir_names.push(name.to_string());
    }
    dir_names.sort();

    for name in &dir_names {
        if !declared.contains(name.as_str()) {
            out.push(ValidationResult {
                level: ValidationLevel::Warning,
                code: "orphan-change-dir",
                message: format!("change directory '{name}' has no plan entry"),
                entry: Some(name.clone()),
            });
        }
    }

    for entry in &plan.changes {
        if entry.status == PlanStatus::InProgress {
            let candidate = changes_dir.join(&entry.name);
            if !candidate.is_dir() {
                out.push(ValidationResult {
                    level: ValidationLevel::Warning,
                    code: "missing-change-dir-for-in-progress",
                    message: format!(
                        "in-progress entry '{}' has no change directory (may briefly be absent during phase start-up)",
                        entry.name
                    ),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::tempdir;

    /// The 10 legal edges from `rfc-2-plan.md` §"Transition Rules".
    /// Kept here (not on `PlanStatus`) so the production matcher and the
    /// test oracle are independent representations of the same table.
    fn allowed_edges() -> HashSet<(PlanStatus, PlanStatus)> {
        use PlanStatus::*;
        let mut set = HashSet::new();
        set.insert((Pending, InProgress));
        set.insert((Pending, Blocked));
        set.insert((Pending, Skipped));
        set.insert((InProgress, Done));
        set.insert((InProgress, Failed));
        set.insert((InProgress, Blocked));
        set.insert((Blocked, Pending));
        set.insert((Failed, Pending));
        set.insert((Failed, Skipped));
        set.insert((Skipped, Pending));
        set
    }

    #[test]
    fn every_legal_edge_transitions_successfully() {
        for (from, to) in allowed_edges() {
            assert!(
                from.can_transition_to(&to),
                "{from:?} -> {to:?} should be allowed by can_transition_to"
            );
            let result = from
                .transition(to)
                .unwrap_or_else(|e| panic!("expected {from:?} -> {to:?} to succeed, got {e:?}"));
            assert_eq!(result, to);
        }
    }

    #[test]
    fn done_is_terminal() {
        for &t in &PlanStatus::ALL {
            assert!(!PlanStatus::Done.can_transition_to(&t), "Done must not allow -> {t:?}");
        }
    }

    #[test]
    fn illegal_edges_rejected() {
        use PlanStatus::*;
        let cases: &[(PlanStatus, PlanStatus)] = &[
            (Done, Pending),
            (Done, InProgress),
            (Done, Failed),
            (Pending, Done),
            (Pending, Failed),
            (Skipped, Failed),
            (InProgress, Pending),
            (InProgress, Skipped),
            (Blocked, Failed),
            (Pending, Pending),
            (InProgress, InProgress),
            (Done, Done),
            (Blocked, Blocked),
            (Failed, Failed),
            (Skipped, Skipped),
        ];

        for &(from, to) in cases {
            assert!(
                !from.can_transition_to(&to),
                "{from:?} -> {to:?} must be rejected by can_transition_to"
            );
            let err = from.transition(to).expect_err(&format!("{from:?} -> {to:?} should be Err"));
            match err {
                Error::PlanTransition { from: f, to: t } => {
                    assert_eq!(f, format!("{from:?}"), "from payload mismatch");
                    assert_eq!(t, format!("{to:?}"), "to payload mismatch");
                }
                other => panic!("expected Error::PlanTransition, got {other:?}"),
            }
        }
    }

    #[test]
    fn exhaustive_table_matches_allowed_set() {
        let allowed = allowed_edges();
        for &from in &PlanStatus::ALL {
            for &to in &PlanStatus::ALL {
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
    fn error_carries_from_and_to() {
        let err = PlanStatus::Done
            .transition(PlanStatus::Pending)
            .expect_err("Done -> Pending must error");
        match err {
            Error::PlanTransition { from, to } => {
                assert_eq!(from, "Done");
                assert_eq!(to, "Pending");
            }
            other => panic!("expected Error::PlanTransition, got {other:?}"),
        }
    }

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

    #[test]
    fn save_then_load_roundtrips_rfc_example() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let original: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        original.save(&path).expect("save ok");
        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, original, "full plan should round-trip through save -> load");
    }

    #[test]
    fn save_creates_new_file_and_emits_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "init".to_string(),
            sources: BTreeMap::new(),
            changes: vec![],
        };
        plan.save(&path).expect("save ok");

        let bytes = std::fs::read(&path).expect("read ok");
        assert!(!bytes.is_empty(), "saved file should not be empty");
        assert_eq!(*bytes.last().unwrap(), b'\n', "saved file should end with a newline");

        let content = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            content.contains("name: init"),
            "file should contain `name: init`, got:\n{content}"
        );
    }

    #[test]
    fn save_overwrites_existing_file_atomically() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "garbage that should be overwritten").expect("write garbage");

        let plan = Plan {
            name: "fresh".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "only-entry".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec![],
                description: None,
                status_reason: None,
            }],
        };
        plan.save(&path).expect("save ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, plan, "loaded plan should equal saved plan");

        let raw = std::fs::read_to_string(&path).expect("read ok");
        assert!(
            !raw.contains("garbage"),
            "pre-existing garbage content should be gone, got:\n{raw}"
        );
    }

    #[test]
    fn load_missing_file_returns_config_error() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.yaml");
        let err = Plan::load(&path).expect_err("expected error on missing file");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("plan.yaml not found"),
                    "message should mention `plan.yaml not found`, got: {msg}"
                );
                assert!(
                    msg.contains(&path.display().to_string()),
                    "message should include the missing path, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn load_tolerates_missing_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "name: foo\nchanges: []").expect("write without trailing newline");
        let plan = Plan::load(&path).expect("load ok");
        assert_eq!(plan.name, "foo");
        assert!(plan.changes.is_empty());
    }

    #[test]
    fn save_writes_kebab_case_on_disk() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "entry-one".to_string(),
                status: PlanStatus::InProgress,
                depends_on: vec!["foo".to_string()],
                affects: vec![],
                sources: vec![],
                description: None,
                status_reason: None,
            }],
        };
        plan.save(&path).expect("save ok");

        let content = std::fs::read_to_string(&path).expect("read ok");
        assert!(
            content.contains("depends-on:"),
            "expected kebab-case `depends-on:`, got:\n{content}"
        );
        assert!(
            content.contains("status: in-progress"),
            "expected kebab-case enum value `in-progress`, got:\n{content}"
        );
        assert!(
            !content.contains("depends_on"),
            "snake_case `depends_on` leaked onto disk, got:\n{content}"
        );
        assert!(
            !content.contains("in_progress"),
            "snake_case `in_progress` leaked onto disk, got:\n{content}"
        );
    }

    fn plan_with_changes(changes: Vec<PlanChange>) -> Plan {
        Plan {
            name: "test".into(),
            sources: BTreeMap::new(),
            changes,
        }
    }

    fn change(name: &str, status: PlanStatus) -> PlanChange {
        PlanChange {
            name: name.into(),
            status,
            depends_on: vec![],
            affects: vec![],
            sources: vec![],
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn clean_plan_returns_no_results() {
        let plan: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let results = plan.validate(None);
        assert!(
            results.is_empty(),
            "expected a clean RFC fixture to validate with no findings, got: {results:#?}"
        );
    }

    #[test]
    fn duplicate_name_reports_error() {
        let plan = plan_with_changes(vec![
            change("foo", PlanStatus::Done),
            change("foo", PlanStatus::Pending),
        ]);
        let results = plan.validate(None);
        let dupes: Vec<_> = results.iter().filter(|r| r.code == "duplicate-name").collect();
        assert_eq!(dupes.len(), 1, "expected one duplicate-name result, got {results:#?}");
        assert_eq!(dupes[0].level, ValidationLevel::Error);
        assert_eq!(dupes[0].entry.as_deref(), Some("foo"));
    }

    #[test]
    fn cycle_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.depends_on = vec!["c".into()];
        let mut b = change("b", PlanStatus::Pending);
        b.depends_on = vec!["a".into()];
        let mut c = change("c", PlanStatus::Pending);
        c.depends_on = vec!["b".into()];
        let plan = plan_with_changes(vec![a, b, c]);
        let results = plan.validate(None);
        let cycles: Vec<_> = results.iter().filter(|r| r.code == "dependency-cycle").collect();
        assert!(!cycles.is_empty(), "expected at least one dependency-cycle, got {results:#?}");
        let msg = &cycles[0].message;
        assert!(msg.contains('a'), "cycle message should name a: {msg}");
        assert!(msg.contains('b'), "cycle message should name b: {msg}");
        assert!(msg.contains('c'), "cycle message should name c: {msg}");
    }

    #[test]
    fn self_cycle_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.depends_on = vec!["a".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None);
        assert!(
            results.iter().any(|r| r.code == "dependency-cycle"),
            "expected a dependency-cycle result for self-edge, got: {results:#?}"
        );
    }

    #[test]
    fn unknown_depends_on_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.depends_on = vec!["bogus".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-depends-on").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-depends-on, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("bogus"));
    }

    #[test]
    fn unknown_affects_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.affects = vec!["ghost".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-affects").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-affects, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("ghost"));
    }

    #[test]
    fn unknown_source_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.sources = vec!["monolith".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-source").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-source, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("monolith"));
    }

    #[test]
    fn multiple_in_progress_reports_error_once_per_offender() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::InProgress),
            change("b", PlanStatus::InProgress),
        ]);
        let results = plan.validate(None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "multiple-in-progress").collect();
        assert_eq!(hits.len(), 2, "expected one result per offender, got {results:#?}");
        let names: HashSet<&str> = hits.iter().filter_map(|r| r.entry.as_deref()).collect();
        assert!(names.contains("a") && names.contains("b"), "names = {names:?}");
    }

    #[test]
    fn single_in_progress_is_fine() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::InProgress),
            change("b", PlanStatus::Pending),
        ]);
        let results = plan.validate(None);
        assert!(
            !results.iter().any(|r| r.code == "multiple-in-progress"),
            "single in-progress entry should not trip multiple-in-progress: {results:#?}"
        );
    }

    #[test]
    fn orphan_change_dir_is_warning() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("stale-change")).expect("mkdir");
        let plan = plan_with_changes(vec![change("other", PlanStatus::Pending)]);
        let results = plan.validate(Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "orphan-change-dir").collect();
        assert_eq!(hits.len(), 1, "expected one orphan-change-dir, got {results:#?}");
        assert_eq!(hits[0].level, ValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("stale-change"));
    }

    #[test]
    fn missing_dir_for_in_progress_is_warning() {
        let tmp = tempdir().expect("tempdir");
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(Some(tmp.path()));
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "missing-change-dir-for-in-progress").collect();
        assert_eq!(hits.len(), 1, "expected one missing-dir warning, got {results:#?}");
        assert_eq!(hits[0].level, ValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("alpha"));
    }

    #[test]
    fn present_dir_for_in_progress_is_silent() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("alpha")).expect("mkdir alpha");
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code.ends_with("-change-dir")
                || r.code == "orphan-change-dir"
                || r.code == "missing-change-dir-for-in-progress"),
            "no directory warnings expected, got: {results:#?}"
        );
    }

    #[test]
    fn changes_dir_none_skips_consistency_checks() {
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(None);
        assert!(
            !results
                .iter()
                .any(|r| r.code == "orphan-change-dir"
                    || r.code == "missing-change-dir-for-in-progress"),
            "passing None for changes_dir must skip directory consistency checks: {results:#?}"
        );
    }

    #[test]
    fn accumulates_all_findings_no_short_circuit() {
        // One plan, three distinct violations:
        //   - duplicate name `foo`
        //   - unknown depends-on target
        //   - unknown source key
        let mut a = change("foo", PlanStatus::Pending);
        a.depends_on = vec!["missing".into()];
        a.sources = vec!["ghost-source".into()];
        let b = change("foo", PlanStatus::Pending);
        let plan = plan_with_changes(vec![a, b]);
        let results = plan.validate(None);

        let codes: HashSet<&'static str> = results.iter().map(|r| r.code).collect();
        for expected in ["duplicate-name", "unknown-depends-on", "unknown-source"] {
            assert!(
                codes.contains(expected),
                "expected code {expected} in {codes:?} — validate must not short-circuit"
            );
        }
    }

    #[test]
    fn save_leaves_no_intermediate_state_observable_after_success() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");

        let first = Plan {
            name: "first".to_string(),
            sources: BTreeMap::new(),
            changes: vec![],
        };
        first.save(&path).expect("save first ok");

        let second = Plan {
            name: "second".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "new-entry".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec![],
                description: None,
                status_reason: None,
            }],
        };
        second.save(&path).expect("save second ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, second, "after a successful save, only the new content is observable");
        assert_ne!(loaded, first, "the previous plan should no longer be on disk");

        let bytes = std::fs::read(&path).expect("read bytes");
        assert!(!bytes.is_empty(), "saved file should not be empty after overwrite");
        assert_eq!(*bytes.last().unwrap(), b'\n', "overwritten file should still end with newline");
    }
}
