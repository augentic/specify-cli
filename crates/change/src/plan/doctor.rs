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
//! ## Stale-clone signature contract
//!
//! RFC-9 §4B's brief explicitly notes that no canonical sync-stamp file
//! is shipped today (4A/4B do not introduce one). So the staleness
//! check uses a *layered* signature design:
//!
//!   1. **Primary, forward-compatible.** Read
//!      `.specify/workspace/<name>/.specify-sync.yaml` (a forward-compatible
//!      stamp file the materialiser may write in a future change). When
//!      present, compare its `url` and `schema` fields to the registry
//!      entry's current `url` / `schema`.
//!   2. **Fallback.** When the stamp file is absent, the *clone's own
//!      git remote URL* (`git remote get-url origin`) is the persisted
//!      sentinel. If it disagrees with the registry's `url`, the clone
//!      pre-dates the most recent `registry add` / hand-edit and the
//!      operator must `specify workspace sync` to resync.
//!   3. **Defensive.** When neither sentinel is readable (no stamp file
//!      *and* `git remote get-url origin` fails or the slot is missing
//!      a `.git/`), the diagnostic is still emitted, with
//!      `reason: missing-sync-stamp`. The brief's defensive contract
//!      ("missing stamps are themselves a warning") drives this branch.
//!
//! Symlink slots are skipped: a symlink always tracks its target tree
//! and "staleness" is not a meaningful concept for them (the registry
//! URL is `.` or a relative path, and the working copy is the live
//! repo).
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
use std::process::Command;

use petgraph::algo::tarjan_scc;
use petgraph::graph::DiGraph;
use serde::{Deserialize, Serialize};
use specify_registry::Registry;

use super::core::{Entry, Finding, Plan, Severity, Status};

/// Stable code for the cycle-detection diagnostic.
///
/// Distinct from validate's `dependency-cycle` so dashboards can route
/// the doctor-only structured payload separately from validate's
/// message-only string.
pub const CODE_CYCLE: &str = "cycle-in-depends-on";
/// Stable code for the orphan-source-key diagnostic — top-level
/// `sources:` key declared but unreferenced by any entry.
pub const CODE_ORPHAN_SOURCE: &str = "orphan-source-key";
/// Stable code for the stale-workspace-clone diagnostic. See
/// [`StaleCloneReason`] for the two ways a clone is classified stale.
pub const CODE_STALE_CLONE: &str = "stale-workspace-clone";
/// Stable code for the unreachable-entry diagnostic — pending entry
/// whose dependency closure is rooted in `failed`/`skipped`.
pub const CODE_UNREACHABLE: &str = "unreachable-entry";

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
    /// Payload for [`CODE_CYCLE`].
    ///
    /// `cycle` is the dependency cycle in stable, alphabetically-sorted
    /// order with the first node repeated at the end so reviewers can
    /// read the loop without mentally closing it.
    Cycle {
        /// Cycle path: `[a, b, c, a]`.
        cycle: Vec<String>,
    },
    /// Payload for [`CODE_ORPHAN_SOURCE`].
    OrphanSource {
        /// Top-level `sources:` key that no entry references.
        key: String,
    },
    /// Payload for [`CODE_STALE_CLONE`].
    StaleClone {
        /// Registry project name whose `.specify/workspace/<project>/`
        /// clone is out of sync.
        project: String,
        /// Why the clone is classified stale.
        reason: StaleCloneReason,
        /// Registry's current signature. `None` is rare (only when the
        /// registry entry could not be re-read mid-check) — emitted so
        /// the JSON shape is stable.
        #[serde(skip_serializing_if = "Option::is_none")]
        expected: Option<CloneSignature>,
        /// Clone's observed signature (sync stamp, or git-remote
        /// fallback). `None` when no sentinel is readable.
        #[serde(skip_serializing_if = "Option::is_none")]
        observed: Option<CloneSignature>,
    },
    /// Payload for [`CODE_UNREACHABLE`].
    UnreachableEntry {
        /// The unreachable plan entry.
        entry: String,
        /// Each immediate `depends-on` predecessor that contributes to
        /// the unreachability — either by being terminal-blocking
        /// (`failed`/`skipped`) or by itself being unreachable.
        blocking: Vec<BlockingPredecessor>,
    },
}

/// Why a workspace clone is classified stale by [`CODE_STALE_CLONE`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StaleCloneReason {
    /// Registry's current `(url, schema)` signature differs from the
    /// signature persisted in the clone (sync stamp file or git
    /// remote URL fallback).
    SignatureChanged,
    /// Neither the sync stamp file nor the git-remote fallback
    /// produced a usable signature for the clone — the staleness
    /// check has nothing to compare against, so doctor warns
    /// defensively.
    MissingSyncStamp,
}

/// Snapshot of the (url, schema) signature for staleness comparison.
/// Either side may be `None` when the corresponding fact is not
/// readable (e.g. clone has no stamp file and no `.git/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CloneSignature {
    /// Repo URL — registry's `url` for the expected signature; sync
    /// stamp's `url` (or `git remote get-url origin`) for observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Schema identifier — registry's `schema` for the expected
    /// signature; sync stamp's `schema` for observed (the git-remote
    /// fallback does not carry schema information).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
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

/// Forward-compatible sync-stamp file the materialiser may persist.
///
/// One file per workspace clone, alongside the rest of the slot's
/// metadata (`.specify/workspace/<name>/.specify-sync.yaml`). Until
/// the materialiser writes it, doctor reads the stamp opportunistically
/// and falls back to the git-remote signature when it is absent. See
/// module docstring §"Stale-clone signature contract".
#[derive(Debug, Clone, Deserialize)]
struct WorkspaceSyncStamp {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    schema: Option<String>,
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
    plan: &Plan, slices_dir: Option<&Path>, registry: Option<&Registry>,
    project_dir: Option<&Path>,
) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> =
        plan.validate(slices_dir, registry).iter().map(Diagnostic::from_finding).collect();

    out.extend(detect_cycles_doctor(&plan.changes));
    out.extend(orphan_source_keys(plan));
    if let (Some(reg), Some(dir)) = (registry, project_dir) {
        out.extend(stale_workspace_clones(reg, dir));
    }
    out.extend(unreachable_entries(&plan.changes));

    out
}

// ---------------------------------------------------------------------------
// 1. Cycle detection (RFC-9 §4B / `cycle-in-depends-on`)
// ---------------------------------------------------------------------------

/// One [`CODE_CYCLE`] diagnostic per cycle in the depends-on graph.
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
            code: CODE_CYCLE.to_string(),
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
    for entry in &plan.changes {
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
            code: CODE_ORPHAN_SOURCE.to_string(),
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

/// Stale-clone diagnostics for every project whose signature drifted.
///
/// Emits one [`CODE_STALE_CLONE`] per registry project whose workspace
/// clone's signature does not match the registry, plus a defensive
/// emission for clones with no readable signature at all. Symlink
/// slots are skipped per the module-level contract.
fn stale_workspace_clones(registry: &Registry, project_dir: &Path) -> Vec<Diagnostic> {
    let workspace_base = project_dir.join(".specify").join("workspace");
    let mut sorted: Vec<&specify_registry::RegistryProject> = registry.projects.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::new();
    for project in sorted {
        if project.url_materialises_as_symlink() {
            continue;
        }
        let slot = workspace_base.join(&project.name);
        if !slot.exists() {
            continue;
        }

        let expected = CloneSignature {
            url: Some(project.url.clone()),
            schema: Some(project.schema.clone()),
        };

        // 1. Forward-compatible stamp file.
        let stamp_path = slot.join(".specify-sync.yaml");
        if let Some(stamp) = read_sync_stamp(&stamp_path) {
            let observed = CloneSignature {
                url: stamp.url.clone(),
                schema: stamp.schema.clone(),
            };
            if expected.url == observed.url && expected.schema == observed.schema {
                continue;
            }
            out.push(diag_signature_changed(&project.name, expected, observed));
            continue;
        }

        // 2. Git-remote fallback.
        let remote_url = git_remote_origin(&slot);
        match remote_url {
            Some(url) => {
                if expected.url.as_deref() == Some(url.as_str()) {
                    continue;
                }
                let observed = CloneSignature {
                    url: Some(url),
                    schema: None,
                };
                out.push(diag_signature_changed(&project.name, expected, observed));
            }
            None => {
                // 3. Defensive — neither sentinel readable.
                out.push(Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: CODE_STALE_CLONE.to_string(),
                    message: format!(
                        "workspace clone '{}' has no `.specify-sync.yaml` and no readable git remote; cannot verify it is in sync with `registry.yaml` — re-run `specify workspace sync` to refresh",
                        project.name
                    ),
                    entry: None,
                    data: Some(DiagnosticPayload::StaleClone {
                        project: project.name.clone(),
                        reason: StaleCloneReason::MissingSyncStamp,
                        expected: Some(expected),
                        observed: None,
                    }),
                });
            }
        }
    }
    out
}

fn diag_signature_changed(
    project: &str, expected: CloneSignature, observed: CloneSignature,
) -> Diagnostic {
    let mut detail_parts = Vec::new();
    if expected.url != observed.url {
        detail_parts.push(format!(
            "url {} -> {}",
            observed.url.clone().unwrap_or_else(|| "<unknown>".to_string()),
            expected.url.clone().unwrap_or_else(|| "<unknown>".to_string())
        ));
    }
    if expected.schema != observed.schema && observed.schema.is_some() {
        detail_parts.push(format!(
            "schema {} -> {}",
            observed.schema.clone().unwrap_or_else(|| "<unknown>".to_string()),
            expected.schema.clone().unwrap_or_else(|| "<unknown>".to_string())
        ));
    }
    let detail = if detail_parts.is_empty() {
        "registry signature has drifted".to_string()
    } else {
        detail_parts.join("; ")
    };
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: CODE_STALE_CLONE.to_string(),
        message: format!(
            "workspace clone '{project}' is out of sync with `registry.yaml` ({detail}); re-run `specify workspace sync`"
        ),
        entry: None,
        data: Some(DiagnosticPayload::StaleClone {
            project: project.to_string(),
            reason: StaleCloneReason::SignatureChanged,
            expected: Some(expected),
            observed: Some(observed),
        }),
    }
}

fn read_sync_stamp(path: &Path) -> Option<WorkspaceSyncStamp> {
    if !path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    serde_saphyr::from_str(&raw).ok()
}

fn git_remote_origin(slot: &Path) -> Option<String> {
    if !slot.join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(slot)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
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
                code: CODE_UNREACHABLE.to_string(),
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
/// [`CODE_CYCLE`].
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

    use specify_registry::RegistryProject;
    use tempfile::tempdir;

    use super::*;
    use crate::plan::core::{Entry, Plan, Status};

    fn change(name: &str, status: Status) -> Entry {
        Entry {
            name: name.into(),
            project: Some("default".into()),
            schema: None,
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
            changes,
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
            changes,
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
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CODE_CYCLE).collect();
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
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CODE_CYCLE).collect();
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
        let count =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CODE_CYCLE).count();
        assert_eq!(count, 2, "expected two distinct cycles");
    }

    #[test]
    fn doctor_cycle_self_loop() {
        let plan = plan_with(vec![change_with_deps("a", Status::Pending, &["a"])]);
        let hits: Vec<_> =
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CODE_CYCLE).collect();
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
            doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CODE_CYCLE).collect();
        assert!(hits.is_empty(), "no cycle expected, got {hits:#?}");
    }

    // ------- 2. Orphan source keys -------------------------------------

    #[test]
    fn doctor_orphan_source_zero() {
        let mut e = change("a", Status::Pending);
        e.sources = vec!["monolith".into()];
        let plan = plan_with_sources(vec![("monolith", "/path")], vec![e]);
        let any_orphan =
            doctor(&plan, None, None, None).into_iter().any(|d| d.code == CODE_ORPHAN_SOURCE);
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
            .filter(|d| d.code == CODE_ORPHAN_SOURCE)
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
            .filter(|d| d.code == CODE_ORPHAN_SOURCE)
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
        let count = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == CODE_ORPHAN_SOURCE)
            .count();
        assert_eq!(count, 1, "only `ghost` should orphan");
    }

    // ------- 3. Unreachable entries ------------------------------------

    #[test]
    fn doctor_unreachable_single_failed_predecessor() {
        let plan = plan_with(vec![
            change("a", Status::Failed),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let hits: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == CODE_UNREACHABLE)
            .collect();
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
        let hits: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == CODE_UNREACHABLE)
            .collect();
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
        let hits: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == CODE_UNREACHABLE)
            .collect();
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
        let unreach: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == CODE_UNREACHABLE)
            .collect();
        let names: Vec<&str> = unreach.iter().filter_map(|d| d.entry.as_deref()).collect();
        assert_eq!(names, vec!["d"], "cycle members must not double-report");
    }

    #[test]
    fn doctor_unreachable_quiet_on_healthy_plan() {
        let plan = plan_with(vec![
            change("a", Status::Done),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let hits: Vec<_> = doctor(&plan, None, None, None)
            .into_iter()
            .filter(|d| d.code == CODE_UNREACHABLE)
            .collect();
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
            schema: schema.into(),
            description: Some(description.into()),
            contracts: None,
        }
    }

    /// Set up a fake project root with a `.specify/workspace/<name>/`
    /// slot wired as a git clone (no remote configured by default).
    fn make_clone_slot(root: &Path, name: &str) -> std::path::PathBuf {
        let slot = root.join(".specify").join("workspace").join(name);
        std::fs::create_dir_all(slot.join(".git")).unwrap();
        slot
    }

    #[test]
    fn doctor_stale_clone_missing_sync_stamp() {
        let tmp = tempdir().unwrap();
        let _slot = make_clone_slot(tmp.path(), "alpha");
        // No `.specify-sync.yaml`, no real git remote — defensive
        // missing-sync-stamp warning.
        let registry = registry_with(vec![rp(
            "alpha",
            "git@github.com:org/alpha.git",
            "omnia@v1",
            "alpha service",
        )]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == CODE_STALE_CLONE)
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
                assert_eq!(*reason, StaleCloneReason::MissingSyncStamp);
                assert!(expected.is_some());
                assert!(observed.is_none());
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[test]
    fn doctor_stale_clone_signature_changed() {
        let tmp = tempdir().unwrap();
        let slot = make_clone_slot(tmp.path(), "alpha");
        // Drop a stamp that disagrees with the registry's url.
        std::fs::write(
            slot.join(".specify-sync.yaml"),
            "url: git@github.com:old/alpha.git\nschema: omnia@v1\n",
        )
        .unwrap();
        let registry = registry_with(vec![rp(
            "alpha",
            "git@github.com:org/alpha.git",
            "omnia@v1",
            "alpha service",
        )]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == CODE_STALE_CLONE)
            .collect();
        assert_eq!(hits.len(), 1);
        match hits[0].data.as_ref().unwrap() {
            DiagnosticPayload::StaleClone {
                reason,
                expected,
                observed,
                ..
            } => {
                assert_eq!(*reason, StaleCloneReason::SignatureChanged);
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
        let slot = make_clone_slot(tmp.path(), "alpha");
        std::fs::write(
            slot.join(".specify-sync.yaml"),
            "url: git@github.com:org/alpha.git\nschema: omnia@v1\n",
        )
        .unwrap();
        let registry = registry_with(vec![rp(
            "alpha",
            "git@github.com:org/alpha.git",
            "omnia@v1",
            "alpha service",
        )]);
        let plan = plan_with(vec![]);
        let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .filter(|d| d.code == CODE_STALE_CLONE)
            .collect();
        assert!(hits.is_empty(), "current signature must not warn, got {hits:#?}");
    }

    #[test]
    fn doctor_stale_clone_skips_symlink_slots() {
        let tmp = tempdir().unwrap();
        // Pretend the registry says url=. (symlink).
        let registry = registry_with(vec![rp("self", ".", "omnia@v1", "self service")]);
        let plan = plan_with(vec![]);
        let any_stale = doctor(&plan, None, Some(&registry), Some(tmp.path()))
            .into_iter()
            .any(|d| d.code == CODE_STALE_CLONE);
        assert!(!any_stale, "symlink slots must not surface stale-clone");
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
        for code in [CODE_CYCLE, CODE_ORPHAN_SOURCE, CODE_STALE_CLONE, CODE_UNREACHABLE] {
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
            diagnostics.iter().any(|d| d.code == CODE_UNREACHABLE),
            "doctor must add the unreachable diagnostic: {diagnostics:#?}"
        );
    }

    #[test]
    fn diagnostic_serialises_kebab_case() {
        let diag = Diagnostic {
            severity: DiagnosticSeverity::Warning,
            code: CODE_ORPHAN_SOURCE.to_string(),
            message: "test".into(),
            entry: None,
            data: Some(DiagnosticPayload::OrphanSource {
                key: "monolith".into(),
            }),
        };
        let v = serde_json::to_value(&diag).expect("serialise");
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["code"], CODE_ORPHAN_SOURCE);
        assert_eq!(v["data"]["kind"], "orphan-source");
        assert_eq!(v["data"]["key"], "monolith");
    }
}
