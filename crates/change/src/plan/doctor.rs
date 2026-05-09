//! `specify plan doctor` — superset of `specify plan validate` plus the
//! four health diagnostics specified by RFC-9 §4B:
//!
//!   - `cycle-in-depends-on`   (error, payload: cycle path)
//!   - `orphan-source-key`     (warning, payload: unreferenced source key)
//!   - `stale-workspace-clone` (warning, payload: project / reason / signatures)
//!   - `unreachable-entry`     (error, payload: blocking predecessors)
//!
//! Doctor is purely additive: it runs every check `Plan::validate` runs,
//! preserves the existing diagnostic codes (`dependency-cycle`,
//! `unknown-depends-on`, `unknown-source`, `multiple-in-progress`,
//! `project-*`, `schema-mismatch-workspace`, …) bit-for-bit, and then
//! layers the four codes above on top with structured payloads. The
//! `Plan::validate` and `Plan::next_eligible` runtime semantics are not
//! changed by anything in this module.
//!
//! ## Stale workspace slot contract
//!
//! RFC-14 C02 made `workspace sync` the authority for whether an
//! existing slot matches `registry.yaml`: remote-backed slots must be
//! git work trees whose `origin` equals the registry URL, and
//! local/relative slots must be symlinks whose canonical target equals
//! the registry target. Doctor reads the same slot-problem inspector
//! from `specify-registry` instead of looking for a speculative
//! `.specify-sync.yaml` stamp. A missing stamp is not a warning; only
//! an actual mismatch that sync would refuse is reported.
//!
//! ## Schema-mismatch overlap
//!
//! `Plan::validate` already emits `schema-mismatch-workspace` when a
//! clone's `.specify/project.yaml:schema` disagrees with the registry's
//! declared schema. Doctor's `stale-workspace-clone` is a *URL* check;
//! the schema check stays on `validate`. Operators see both signals
//! when the clone is out of sync on both axes, and the codes are
//! orthogonal so dashboards can route each to the right runbook.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use petgraph::algo::tarjan_scc;
use petgraph::graph::DiGraph;
use serde::{Deserialize, Serialize};
use specify_registry::workspace::{SlotProblem, SlotProblemReason, slot_problem};
use specify_registry::{Registry, RegistryProject};

use super::core::{Entry, Finding, Plan, Severity, Status};

/// Stable code for the cycle-detection diagnostic.
///
/// Distinct from validate's `dependency-cycle` so dashboards can route
/// the doctor-only structured payload separately from validate's
/// message-only string.
pub const CYCLE: &str = "cycle-in-depends-on";
/// Stable code for the orphan-source-key diagnostic — top-level
/// `sources:` key declared but unreferenced by any entry.
pub const ORPHAN_SOURCE: &str = "orphan-source-key";
/// Stable code for the stale-workspace-clone diagnostic. See
/// [`StaleReason`] for the two ways a clone is classified stale.
pub const STALE_CLONE: &str = "stale-workspace-clone";
/// Stable code for the unreachable-entry diagnostic — pending entry
/// whose dependency closure is rooted in `failed`/`skipped`.
pub const UNREACHABLE: &str = "unreachable-entry";

/// One row in the doctor diagnostic stream.
///
/// Wire shape (kebab-case):
///
/// ```json
/// {
///   "severity": "error" | "warning",
///   "code": "<stable code>",
///   "message": "<human readable>",
///   "entry": null | "<plan entry name>",
///   "data": null | { ... payload ... }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Diagnostic {
    /// Severity bucket.
    pub severity: DiagnosticSeverity,
    /// Stable machine-readable code. The four doctor-only codes are the
    /// constants on this module (`CODE_*`); validate's codes come
    /// through unchanged.
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Offending plan entry name when the finding is entry-local;
    /// `None` for plan-wide findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Structured payload — `Some` only on the four doctor-specific
    /// codes; `None` for findings forwarded from `Plan::validate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<DiagnosticPayload>,
}

/// JSON-shape mirror of [`Severity`] with kebab-case casing for wire
/// output.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSeverity {
    /// Blocking problem.
    Error,
    /// Non-blocking advisory.
    Warning,
}

impl DiagnosticSeverity {
    /// Fixed wire string. Matches the serde `kebab-case` output and
    /// the `<severity>:` prefix in text mode.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

impl From<&Severity> for DiagnosticSeverity {
    fn from(value: &Severity) -> Self {
        match value {
            Severity::Error => Self::Error,
            Severity::Warning => Self::Warning,
        }
    }
}

/// Structured payload carried by the four doctor-only diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DiagnosticPayload {
    /// Payload for [`CYCLE`].
    ///
    /// `cycle` is the dependency cycle in stable, alphabetically-sorted
    /// order with the first node repeated at the end so reviewers can
    /// read the loop without mentally closing it.
    Cycle {
        /// Cycle path: `[a, b, c, a]`.
        cycle: Vec<String>,
    },
    /// Payload for [`ORPHAN_SOURCE`].
    OrphanSource {
        /// Top-level `sources:` key that no entry references.
        key: String,
    },
    /// Payload for [`STALE_CLONE`].
    StaleClone {
        /// Registry project name whose `.specify/workspace/<project>/`
        /// slot is out of sync.
        project: String,
        /// Why the slot is classified stale.
        reason: StaleReason,
        /// Registry's expected signature for the slot.
        #[serde(skip_serializing_if = "Option::is_none")]
        expected: Option<CloneSignature>,
        /// Slot's observed signature, when inspectable.
        #[serde(skip_serializing_if = "Option::is_none")]
        observed: Option<CloneSignature>,
    },
    /// Payload for [`UNREACHABLE`].
    UnreachableEntry {
        /// The unreachable plan entry.
        entry: String,
        /// Each immediate `depends-on` predecessor that contributes to
        /// the unreachability — either by being terminal-blocking
        /// (`failed`/`skipped`) or by itself being unreachable.
        blocking: Vec<BlockingPredecessor>,
    },
}

/// Why a workspace clone is classified stale by [`STALE_CLONE`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StaleReason {
    /// A remote-backed clone's `origin` differs from the registry URL.
    SignatureChanged,
    /// Slot materialisation does not match the registry URL class or target.
    SlotMismatch,
    /// Retained for old JSON consumers. RFC-14 doctor no longer emits
    /// this reason because sync does not write `.specify-sync.yaml`.
    MissingSyncStamp,
}

/// Snapshot of the registry or slot signature for staleness comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CloneSignature {
    /// Materialisation kind (`git-clone`, `symlink`, or `other`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot_kind: Option<String>,
    /// Repo URL — registry's `url` for the expected signature; git
    /// `origin` for observed remote-backed slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Capability identifier from the registry's `capability` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Canonical filesystem target for symlink-backed slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// One immediate predecessor of an unreachable entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BlockingPredecessor {
    /// Predecessor plan-entry name.
    pub name: String,
    /// Predecessor's current plan-entry status (always one of
    /// `failed`, `skipped`, or `pending` — pending appears when the
    /// predecessor is itself unreachable; the chain is reported via
    /// the predecessor's own `unreachable-entry` diagnostic).
    pub status: String,
}

impl Diagnostic {
    /// Forward a `Plan::validate` finding to the doctor stream
    /// without payload data, preserving the original code and
    /// severity.
    fn from_finding(f: &Finding) -> Self {
        Self {
            severity: DiagnosticSeverity::from(&f.level),
            code: f.code.to_string(),
            message: f.message.clone(),
            entry: f.entry.clone(),
            data: None,
        }
    }
}

/// Run every `Plan::validate` check, then layer the four RFC-9 §4B
/// diagnostics on top.
///
/// `slices_dir` and `registry` are forwarded to `Plan::validate` so
/// the validate-level findings are bit-identical to those emitted by
/// `specify plan validate`. `project_dir` is consulted only by the
/// stale-workspace-clone check; pass `None` to skip that check
/// (`Plan::doctor_pure` does the same — see the unit tests).
///
/// The order in the returned vector is stable:
///
///   1. Every `Plan::validate` finding, in the existing order.
///   2. Cycle diagnostics (one per cycle, deduplicated by node-set).
///   3. Orphan source-key diagnostics (sorted by key).
///   4. Stale workspace clone diagnostics (sorted by project name).
///   5. Unreachable-entry diagnostics (sorted by entry name).
#[must_use]
pub fn doctor(
    plan: &Plan, slices_dir: Option<&Path>, registry: Option<&Registry>, project_dir: Option<&Path>,
) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> =
        plan.validate(slices_dir, registry).iter().map(Diagnostic::from_finding).collect();

    out.extend(detect_cycles_doctor(&plan.entries));
    out.extend(orphan_source_keys(plan));
    if let (Some(reg), Some(dir)) = (registry, project_dir) {
        out.extend(stale_workspace_clones(reg, dir));
    }
    out.extend(unreachable_entries(&plan.entries));

    out
}

// ---------------------------------------------------------------------------
// 1. Cycle detection (RFC-9 §4B / `cycle-in-depends-on`)
// ---------------------------------------------------------------------------

/// One [`CYCLE`] diagnostic per cycle in the depends-on graph.
///
/// Self-loops are emitted too. Cycles are deduplicated by sorted
/// node-set so every distinct cycle surfaces exactly once. The cycle
/// path is sorted alphabetically with the first node repeated at the
/// end — matches the convention used by validate's `dependency-cycle`
/// text.
fn detect_cycles_doctor(changes: &[Entry]) -> Vec<Diagnostic> {
    let mut graph: DiGraph<&str, ()> = DiGraph::new();
    let mut idx = HashMap::new();
    for entry in changes {
        let node = graph.add_node(entry.name.as_str());
        idx.insert(entry.name.as_str(), node);
    }
    for entry in changes {
        let to = idx[entry.name.as_str()];
        for dep in &entry.depends_on {
            if let Some(&from) = idx.get(dep.as_str()) {
                graph.add_edge(from, to, ());
            }
        }
    }

    let mut out = Vec::new();
    for scc in tarjan_scc(&graph) {
        let cycle_names: Vec<String> = match scc.len() {
            0 => continue,
            1 => {
                let n = scc[0];
                if graph.find_edge(n, n).is_some() {
                    vec![graph[n].to_string(), graph[n].to_string()]
                } else {
                    continue;
                }
            }
            _ => {
                let mut names: Vec<String> = scc.iter().map(|&n| graph[n].to_string()).collect();
                names.sort_unstable();
                let head = names[0].clone();
                names.push(head);
                names
            }
        };
        let pretty = cycle_names.join(" → ");
        out.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: CYCLE.to_string(),
            message: format!("dependency cycle: {pretty}"),
            entry: None,
            data: Some(DiagnosticPayload::Cycle { cycle: cycle_names }),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 2. Orphan source keys (RFC-9 §4B / `orphan-source-key`)
// ---------------------------------------------------------------------------

/// Top-level `sources:` keys declared but not referenced by any entry.
///
/// The inverse of validate's `unknown-source`, which catches *entry
/// references* to undeclared keys; this catches *declarations* with no
/// references.
fn orphan_source_keys(plan: &Plan) -> Vec<Diagnostic> {
    let mut referenced: HashSet<&str> = HashSet::new();
    for entry in &plan.entries {
        for k in &entry.sources {
            referenced.insert(k.as_str());
        }
    }
    let mut orphans: Vec<&str> = plan
        .sources
        .keys()
        .filter(|k| !referenced.contains(k.as_str()))
        .map(String::as_str)
        .collect();
    orphans.sort_unstable();
    orphans
        .into_iter()
        .map(|key| Diagnostic {
            severity: DiagnosticSeverity::Warning,
            code: ORPHAN_SOURCE.to_string(),
            message: format!(
                "source key '{key}' is declared in the plan-level `sources:` map but no entry references it; either reference it from an entry's `sources:` list or remove the declaration"
            ),
            entry: None,
            data: Some(DiagnosticPayload::OrphanSource {
                key: key.to_string(),
            }),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 3. Stale workspace clones (RFC-9 §4B / `stale-workspace-clone`)
// ---------------------------------------------------------------------------

/// Stale-slot diagnostics for every project whose materialisation drifted.
///
/// Emits one [`STALE_CLONE`] per registry project whose existing workspace
/// slot would be refused by `workspace sync`. Missing slots are left to
/// `workspace sync`; absent `.specify-sync.yaml` metadata is ignored.
fn stale_workspace_clones(registry: &Registry, project_dir: &Path) -> Vec<Diagnostic> {
    let mut sorted: Vec<&RegistryProject> = registry.projects.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::new();
    for project in sorted {
        if let Some(problem) = slot_problem(project_dir, project) {
            out.push(diag_slot_problem(project, &problem));
        }
    }
    out
}

fn diag_slot_problem(project: &RegistryProject, problem: &SlotProblem) -> Diagnostic {
    let expected = CloneSignature {
        slot_kind: Some(problem.expected_kind.label().to_string()),
        url: Some(project.url.clone()),
        capability: Some(project.capability.clone()),
        target: problem.expected_target.as_ref().map(|path| path.display().to_string()),
    };
    let observed = CloneSignature {
        slot_kind: problem.observed_kind.map(|kind| kind.label().to_string()),
        url: problem.observed_url.clone(),
        capability: None,
        target: problem.observed_target.as_ref().map(|path| path.display().to_string()),
    };
    let reason = if problem.reason == SlotProblemReason::RemoteOriginMismatch {
        StaleReason::SignatureChanged
    } else {
        StaleReason::SlotMismatch
    };
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: STALE_CLONE.to_string(),
        message: format!(
            "workspace slot '{}' is out of sync with `registry.yaml`: {}",
            project.name,
            problem.message()
        ),
        entry: None,
        data: Some(DiagnosticPayload::StaleClone {
            project: project.name.clone(),
            reason,
            expected: Some(expected),
            observed: Some(observed),
        }),
    }
}

// ---------------------------------------------------------------------------
// 4. Unreachable entries (RFC-9 §4B / `unreachable-entry`)
// ---------------------------------------------------------------------------

/// Pending entries whose dependency closure is rooted in a terminal blocker.
///
/// Terminal blockers are entries with status `failed` or `skipped`.
///
/// Algorithm: fixpoint walk.
///
///   1. `cycles` = set of entry names that participate in a cycle (so
///      we do not double-report them as both cyclic and unreachable).
///   2. Seed `unreachable` with every entry whose status is
///      `failed`/`skipped` — they're not Pending themselves but they
///      are the upstream blockers we propagate from.
///   3. Iterate: for every Pending entry P not in `cycles` and not yet
///      in `unreachable`, mark it unreachable when *any* immediate
///      `depends-on` predecessor is in `unreachable`.
///   4. Stop when no entry was added in the last pass.
///   5. Emit a diagnostic for every Pending entry that landed in
///      `unreachable`. The `blocking` payload lists immediate
///      predecessors that are themselves in `unreachable` — i.e. the
///      proximate cause(s) of P's unreachability.
fn unreachable_entries(changes: &[Entry]) -> Vec<Diagnostic> {
    let cycles = cycle_membership(changes);

    let by_name: HashMap<&str, &Entry> = changes.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut unreachable: HashSet<String> = HashSet::new();
    for entry in changes {
        if matches!(entry.status, Status::Failed | Status::Skipped) {
            unreachable.insert(entry.name.clone());
        }
    }

    loop {
        let mut grew = false;
        for entry in changes {
            if entry.status != Status::Pending {
                continue;
            }
            if cycles.contains(entry.name.as_str()) {
                continue;
            }
            if unreachable.contains(&entry.name) {
                continue;
            }
            let blocked = entry.depends_on.iter().any(|dep| unreachable.contains(dep));
            if blocked {
                unreachable.insert(entry.name.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let mut hits: Vec<&Entry> = changes
        .iter()
        .filter(|e| {
            e.status == Status::Pending
                && unreachable.contains(&e.name)
                && !cycles.contains(e.name.as_str())
        })
        .collect();
    hits.sort_by(|a, b| a.name.cmp(&b.name));

    hits.into_iter()
        .map(|entry| {
            let blocking: Vec<BlockingPredecessor> = entry
                .depends_on
                .iter()
                .filter_map(|dep| {
                    if !unreachable.contains(dep) {
                        return None;
                    }
                    let status = by_name
                        .get(dep.as_str())
                        .map_or_else(|| "unknown".to_string(), |e| e.status.to_string());
                    Some(BlockingPredecessor {
                        name: dep.clone(),
                        status,
                    })
                })
                .collect();
            let detail = blocking
                .iter()
                .map(|b| format!("{} ({})", b.name, b.status))
                .collect::<Vec<_>>()
                .join(", ");
            Diagnostic {
                severity: DiagnosticSeverity::Error,
                code: UNREACHABLE.to_string(),
                message: format!("entry '{}' is unreachable: blocked by {}", entry.name, detail),
                entry: Some(entry.name.clone()),
                data: Some(DiagnosticPayload::UnreachableEntry {
                    entry: entry.name.clone(),
                    blocking,
                }),
            }
        })
        .collect()
}

/// Return the set of entry names that participate in any cycle.
///
/// Self-loops are included. Used by the unreachable check to avoid
/// double-reporting entries that are already surfaced under
/// [`CYCLE`].
fn cycle_membership(changes: &[Entry]) -> HashSet<&str> {
    let mut graph: DiGraph<&str, ()> = DiGraph::new();
    let mut idx = HashMap::new();
    for entry in changes {
        let node = graph.add_node(entry.name.as_str());
        idx.insert(entry.name.as_str(), node);
    }
    for entry in changes {
        let to = idx[entry.name.as_str()];
        for dep in &entry.depends_on {
            if let Some(&from) = idx.get(dep.as_str()) {
                graph.add_edge(from, to, ());
            }
        }
    }

    let mut members: HashSet<&str> = HashSet::new();
    for scc in tarjan_scc(&graph) {
        if scc.len() > 1 {
            for n in scc {
                members.insert(graph[n]);
            }
        } else if scc.len() == 1 {
            let n = scc[0];
            if graph.find_edge(n, n).is_some() {
                members.insert(graph[n]);
            }
        }
    }
    members
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use specify_registry::RegistryProject;
    use tempfile::tempdir;

    use super::*;
    use crate::plan::core::{Entry, Plan, Status};

    fn change(name: &str, status: Status) -> Entry {
        Entry {
            name: name.into(),
            project: Some("default".into()),
            capability: None,
            status,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }
    }

    fn change_with_deps(name: &str, status: Status, deps: &[&str]) -> Entry {
        let mut e = change(name, status);
        e.depends_on = deps.iter().map(|s| (*s).to_string()).collect();
        e
    }

    fn plan_with(changes: Vec<Entry>) -> Plan {
        Plan {
            name: "test".into(),
            sources: BTreeMap::new(),
            entries: changes,
        }
    }

    fn plan_with_sources(sources: Vec<(&str, &str)>, changes: Vec<Entry>) -> Plan {
        let mut map = BTreeMap::new();
        for (k, v) in sources {
            map.insert(k.to_string(), v.to_string());
        }
        Plan {
            name: "test".into(),
            sources: map,
            entries: changes,
        }
    }

    // ------- 1. Cycle detection ----------------------------------------

    #[test]
    fn doctor_cycle_two_node() {
        let plan = plan_with(vec![
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
        assert_eq!(hits.len(), 1, "expected one cycle, got {hits:#?}");
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::Cycle { cycle } => {
                assert_eq!(cycle, &vec!["a".to_string(), "b".to_string(), "a".to_string()]);
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_cycle_three_node() {
        let plan = plan_with(vec![
            change_with_deps("a", Status::Pending, &["c"]),
            change_with_deps("b", Status::Pending, &["a"]),
            change_with_deps("c", Status::Pending, &["b"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
        assert_eq!(hits.len(), 1, "single SCC, single diagnostic");
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::Cycle { cycle } => {
                assert_eq!(
                    cycle,
                    &vec!["a".to_string(), "b".to_string(), "c".to_string(), "a".to_string()]
                );
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_cycle_two_disjoint() {
        let plan = plan_with(vec![
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
            change_with_deps("c", Status::Pending, &["d"]),
            change_with_deps("d", Status::Pending, &["c"]),
        ]);
        let count = doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).count();
        assert_eq!(count, 2, "expected two distinct cycles");
    }

    #[test]
    fn doctor_cycle_self_loop() {
        let plan = plan_with(vec![change_with_deps("a", Status::Pending, &["a"])]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
        assert_eq!(hits.len(), 1);
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::Cycle { cycle } => {
                assert_eq!(cycle, &vec!["a".to_string(), "a".to_string()]);
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_no_cycle_quiet() {
        let plan = plan_with(vec![
            change("a", Status::Done),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
        assert!(hits.is_empty(), "no cycle expected, got {hits:#?}");
    }

    // ------- 2. Orphan source keys -------------------------------------

    #[test]
    fn doctor_orphan_source_zero() {
        let mut e = change("a", Status::Pending);
        e.sources = vec!["monolith".into()];
        let plan = plan_with_sources(vec![("monolith", "/path")], vec![e]);
        let any_orphan =
            doctor(&plan, None, None, None).into_iter().any(|d| d.code == ORPHAN_SOURCE);
        assert!(!any_orphan);
    }

    #[test]
    fn doctor_orphan_source_one() {
        let plan = plan_with_sources(
            vec![("monolith", "/path"), ("orphan", "/elsewhere")],
            vec![{
                let mut e = change("a", Status::Pending);
                e.sources = vec!["monolith".into()];
                e
            }],
        );
        let hits: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == ORPHAN_SOURCE)
            .collect();
        assert_eq!(hits.len(), 1);
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::OrphanSource { key } => assert_eq!(key, "orphan"),
            other => panic!("wrong payload: {other:?}"),
        }
        assert_eq!(hits[0].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn doctor_orphan_source_multiple_sorted() {
        let plan = plan_with_sources(
            vec![("alpha", "/a"), ("beta", "/b"), ("gamma", "/g"), ("monolith", "/m")],
            vec![{
                let mut e = change("a", Status::Pending);
                e.sources = vec!["monolith".into()];
                e
            }],
        );
        let hits: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == ORPHAN_SOURCE)
            .collect();
        let keys: Vec<&str> = hits
            .iter()
            .map(|d| match d.data.as_ref().unwrap() {
                DiagnosticPayload::OrphanSource { key } => key.as_str(),
                _ => panic!("wrong payload"),
            })
            .collect();
        assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn doctor_orphan_source_mixed_references() {
        let plan = plan_with_sources(
            vec![("monolith", "/m"), ("orders", "/o"), ("ghost", "/g")],
            vec![
                {
                    let mut e = change("a", Status::Pending);
                    e.sources = vec!["monolith".into(), "orders".into()];
                    e
                },
                {
                    let mut e = change("b", Status::Done);
                    e.sources = vec!["orders".into()];
                    e
                },
            ],
        );
        let count =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == ORPHAN_SOURCE).count();
        assert_eq!(count, 1, "only `ghost` should orphan");
    }

    // ------- 3. Unreachable entries ------------------------------------

    #[test]
    fn doctor_unreachable_single_failed_predecessor() {
        let plan = plan_with(vec![
            change("a", Status::Failed),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.as_deref(), Some("b"));
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::UnreachableEntry { entry, blocking } => {
                assert_eq!(entry, "b");
                assert_eq!(blocking.len(), 1);
                assert_eq!(blocking[0].name, "a");
                assert_eq!(blocking[0].status, "failed");
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_unreachable_transitive_failure() {
        let plan = plan_with(vec![
            change("a", Status::Failed),
            change_with_deps("b", Status::Pending, &["a"]),
            change_with_deps("c", Status::Pending, &["b"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
        let names: Vec<&str> = hits.iter().filter_map(|d| d.entry.as_deref()).collect();
        assert_eq!(names, vec!["b", "c"], "both b and c are unreachable, sorted");
        // c's blocking points at b (Pending and unreachable).
        let c = hits.iter().find(|d| d.entry.as_deref() == Some("c")).unwrap();
        match c.data.as_ref().unwrap() {
            DiagnosticPayload::UnreachableEntry { blocking, .. } => {
                assert_eq!(blocking.len(), 1);
                assert_eq!(blocking[0].name, "b");
                assert_eq!(blocking[0].status, "pending");
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_unreachable_mixed_terminal_predecessors() {
        let plan = plan_with(vec![
            change("a", Status::Failed),
            change("b", Status::Skipped),
            change_with_deps("c", Status::Pending, &["a", "b"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.as_deref(), Some("c"));
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::UnreachableEntry { blocking, .. } => {
                let mut names: Vec<&str> = blocking.iter().map(|b| b.name.as_str()).collect();
                names.sort_unstable();
                assert_eq!(names, vec!["a", "b"]);
                let mut statuses: Vec<&str> = blocking.iter().map(|b| b.status.as_str()).collect();
                statuses.sort_unstable();
                assert_eq!(statuses, vec!["failed", "skipped"]);
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_unreachable_skips_cycle_members() {
        // a-b cycle plus c-failed -> d-pending. Only d should be
        // reported as unreachable; a/b show up under cycle-in-depends-on.
        let plan = plan_with(vec![
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
            change("c", Status::Failed),
            change_with_deps("d", Status::Pending, &["c"]),
        ]);
        let unreach: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
        let names: Vec<&str> = unreach.iter().filter_map(|d| d.entry.as_deref()).collect();
        assert_eq!(names, vec!["d"], "cycle members must not double-report");
    }

    #[test]
    fn doctor_unreachable_quiet_on_healthy_plan() {
        let plan = plan_with(vec![
            change("a", Status::Done),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
        assert!(hits.is_empty(), "no unreachable expected, got {hits:#?}");
    }

    // ------- 4. Stale workspace clones --------------------------------

    fn registry_with(projects: Vec<RegistryProject>) -> Registry {
        Registry { version: 1, projects }
    }

    fn rp(name: &str, url: &str, schema: &str, description: &str) -> RegistryProject {
        RegistryProject {
            name: name.into(),
            url: url.into(),
            capability: schema.into(),
            description: Some(description.into()),
            contracts: None,
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git").arg("-C").arg(cwd).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git -C {} {} failed\nstdout:\n{}\nstderr:\n{}",
            cwd.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Set up a project root with a `.specify/workspace/<name>/`
    /// slot wired as a git clone.
    fn make_clone_slot(root: &Path, name: &str, origin: Option<&str>) -> std::path::PathBuf {
        let slot = root.join(".specify").join("workspace").join(name);
        std::fs::create_dir_all(&slot).unwrap();
        run_git(&slot, &["init"]);
        if let Some(origin) = origin {
            run_git(&slot, &["remote", "add", "origin", origin]);
        }
        slot
    }

    #[cfg(unix)]
    fn symlink_dir(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn symlink_dir(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_dir(target, link).unwrap();
    }

    #[test]
    fn doctor_stale_clone_reports_missing_origin_without_sync_stamp_warning() {
        let tmp = tempdir().unwrap();
        let _slot = make_clone_slot(tmp.path(), "alpha", None);
        let registry = registry_with(vec![rp(
            "alpha",
            "git@github.com:org/alpha.git",
            "omnia@v1",
            "alpha service",
        )]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == STALE_CLONE)
            .collect();
        assert_eq!(hits.len(), 1, "expected single stale-clone, got {hits:#?}");
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::StaleClone {
                project,
                reason,
                expected,
                observed,
            } => {
                assert_eq!(project, "alpha");
                assert_eq!(*reason, StaleReason::SlotMismatch);
                assert_eq!(expected.as_ref().unwrap().slot_kind.as_deref(), Some("git-clone"));
                assert_eq!(observed.as_ref().unwrap().slot_kind.as_deref(), Some("git-clone"));
                assert!(observed.as_ref().unwrap().url.is_none());
            }
            other => panic!("wrong payload: {other:?}"),
        }
        assert!(
            hits[0].message.contains("has no origin remote"),
            "missing origin should be reported via sync slot rules: {:?}",
            hits[0].message
        );
    }

    #[test]
    fn doctor_stale_clone_signature_changed() {
        let tmp = tempdir().unwrap();
        make_clone_slot(tmp.path(), "alpha", Some("git@github.com:old/alpha.git"));
        let registry = registry_with(vec![rp(
            "alpha",
            "git@github.com:org/alpha.git",
            "omnia@v1",
            "alpha service",
        )]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == STALE_CLONE)
            .collect();
        assert_eq!(hits.len(), 1);
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::StaleClone {
                reason,
                expected,
                observed,
                ..
            } => {
                assert_eq!(*reason, StaleReason::SignatureChanged);
                assert_eq!(
                    expected.as_ref().unwrap().url.as_deref(),
                    Some("git@github.com:org/alpha.git")
                );
                assert_eq!(
                    observed.as_ref().unwrap().url.as_deref(),
                    Some("git@github.com:old/alpha.git")
                );
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_stale_clone_signature_current() {
        let tmp = tempdir().unwrap();
        make_clone_slot(tmp.path(), "alpha", Some("git@github.com:org/alpha.git"));
        let registry = registry_with(vec![rp(
            "alpha",
            "git@github.com:org/alpha.git",
            "omnia@v1",
            "alpha service",
        )]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == STALE_CLONE)
            .collect();
        assert!(hits.is_empty(), "current signature must not warn, got {hits:#?}");
    }

    #[test]
    fn doctor_stale_clone_diagnoses_wrong_symlink_target() {
        let tmp = tempdir().unwrap();
        let peer = tmp.path().join("peer");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&peer).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let workspace = tmp.path().join(".specify").join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        symlink_dir(&other, &workspace.join("peer"));
        let registry = registry_with(vec![rp("peer", "./peer", "omnia@v1", "peer service")]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == STALE_CLONE)
            .collect();
        assert_eq!(hits.len(), 1, "wrong symlink target must surface stale slot");
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::StaleClone {
                reason,
                expected,
                observed,
                ..
            } => {
                assert_eq!(*reason, StaleReason::SlotMismatch);
                assert_eq!(expected.as_ref().unwrap().slot_kind.as_deref(), Some("symlink"));
                assert_eq!(observed.as_ref().unwrap().slot_kind.as_deref(), Some("symlink"));
                assert!(
                    observed.as_ref().unwrap().target.as_ref().unwrap().contains("other"),
                    "observed target should name the wrong symlink target"
                );
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_stale_clone_ignores_missing_symlink_slots() {
        let tmp = tempdir().unwrap();
        let registry = registry_with(vec![rp("self", ".", "omnia@v1", "self service")]);
        let plan = plan_with(vec![]);
        let any_stale = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .any(|d| d.code == STALE_CLONE);
        assert!(!any_stale, "missing slots are left to workspace sync");
    }

    // ------- Combined / negative cases --------------------------------

    #[test]
    fn doctor_healthy_plan_emits_zero_doctor_diagnostics() {
        let plan = plan_with_sources(
            vec![("monolith", "/m")],
            vec![
                {
                    let mut e = change("a", Status::Done);
                    e.sources = vec!["monolith".into()];
                    e
                },
                {
                    let mut e = change_with_deps("b", Status::Pending, &["a"]);
                    e.sources = vec!["monolith".into()];
                    e
                },
            ],
        );
        let diagnostics = doctor(&plan, None, None, None);
        for code in [CYCLE, ORPHAN_SOURCE, STALE_CLONE, UNREACHABLE] {
            assert!(
                !diagnostics.iter().any(|d| d.code == code),
                "healthy plan should not emit {code}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn doctor_includes_validate_findings_unchanged() {
        // A plan with both an unknown depends-on (validate-only) and a
        // failed predecessor (doctor-only). Doctor must surface BOTH
        // diagnostics, with validate's code unchanged.
        let plan = plan_with(vec![
            change("a", Status::Failed),
            change_with_deps("b", Status::Pending, &["a", "ghost"]),
        ]);
        let diagnostics = doctor(&plan, None, None, None);
        assert!(
            diagnostics.iter().any(|d| d.code == "unknown-depends-on"),
            "validate's `unknown-depends-on` must pass through doctor unchanged: {diagnostics:#?}"
        );
        assert!(
            diagnostics.iter().any(|d| d.code == UNREACHABLE),
            "doctor must add the unreachable diagnostic: {diagnostics:#?}"
        );
    }

    #[test]
    fn diagnostic_serialises_kebab_case() {
        let diag = Diagnostic {
            severity: DiagnosticSeverity::Warning,
            code: ORPHAN_SOURCE.to_string(),
            message: "test".into(),
            entry: None,
            data: Some(DiagnosticPayload::OrphanSource {
                key: "monolith".into(),
            }),
        };
        let v = serde_json::to_value(&diag).expect("serialise");
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["code"], ORPHAN_SOURCE);
        assert_eq!(v["data"]["kind"], "orphan-source");
        assert_eq!(v["data"]["key"], "monolith");
    }
}
