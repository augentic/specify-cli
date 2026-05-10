//! [`Plan::validate`] and the per-check helpers it composes.
//!
//! Findings are accumulated — no check short-circuits another. Order
//! is structural checks first followed by consistency checks against
//! `slices_dir` and the registry.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::DiGraph;
use specify_registry::Registry;

use super::model::{Entry, Finding, Plan, Severity, Status};

impl Plan {
    /// Run all structural and semantic checks over the plan.
    ///
    /// `slices_dir` (when `Some`) points at `.specify/slices/` and
    /// enables the cross-reference checks against on-disk slice
    /// metadata. `registry` (when `Some`) enables the cross-registry
    /// checks (`project-not-in-registry`, `project-missing-multi-repo`).
    ///
    /// Findings are accumulated — no check short-circuits another. Order
    /// is structural checks first (duplicate names, cycles, unknown
    /// depends-on / sources, multiple in-progress) followed by
    /// consistency checks against `slices_dir` when provided.
    ///
    /// Note on "well-formed status values": `Status` is an enum, so
    /// every in-memory instance is well-formed by construction. serde
    /// rejects invalid statuses at parse time, which is not reachable
    /// in-process — so nothing is emitted for it.
    #[must_use]
    pub fn validate(&self, slices_dir: Option<&Path>, registry: Option<&Registry>) -> Vec<Finding> {
        let mut results = Vec::new();
        results.extend(duplicate_names(&self.entries));
        results.extend(detect_cycles(&self.entries));
        results.extend(check_unknown_depends_on(&self.entries));
        results.extend(check_unknown_sources(self));
        results.extend(check_single_in_progress(&self.entries));
        results.extend(missing_project_or_schema(&self.entries));
        results.extend(check_context_paths(&self.entries));
        if let Some(reg) = registry {
            results.extend(check_project_in_registry(&self.entries, reg));
            results.extend(check_project_required_multi_repo(&self.entries, reg));
        }
        if let Some(dir) = slices_dir.filter(|d| d.is_dir()) {
            results.extend(slices_dir_consistency(self, dir));
        }
        results
    }
}

fn duplicate_names(changes: &[Entry]) -> Vec<Finding> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for entry in changes {
        if !seen.insert(entry.name.as_str()) {
            out.push(Finding {
                level: Severity::Error,
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
fn detect_cycles(changes: &[Entry]) -> Vec<Finding> {
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
            out.push(Finding {
                level: Severity::Error,
                code: "dependency-cycle",
                message: format!("cycle: {}", path.join(" → ")),
                entry: None,
            });
        } else if scc.len() == 1 {
            let node = scc[0];
            if graph.find_edge(node, node).is_some() {
                let name = graph[node];
                out.push(Finding {
                    level: Severity::Error,
                    code: "dependency-cycle",
                    message: format!("cycle: {name} → {name}"),
                    entry: None,
                });
            }
        }
    }
    out
}

fn check_unknown_depends_on(changes: &[Entry]) -> Vec<Finding> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.depends_on {
            if !known.contains(target.as_str()) {
                out.push(Finding {
                    level: Severity::Error,
                    code: "unknown-depends-on",
                    message: format!("depends-on references unknown change '{target}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

fn check_unknown_sources(plan: &Plan) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in &plan.entries {
        for key in &entry.sources {
            if !plan.sources.contains_key(key) {
                out.push(Finding {
                    level: Severity::Error,
                    code: "unknown-source",
                    message: format!("sources references unknown source key '{key}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

fn check_single_in_progress(changes: &[Entry]) -> Vec<Finding> {
    let offenders: Vec<&Entry> =
        changes.iter().filter(|c| c.status == Status::InProgress).collect();
    if offenders.len() <= 1 {
        return Vec::new();
    }
    offenders
        .into_iter()
        .map(|c| Finding {
            level: Severity::Error,
            code: "multiple-in-progress",
            message: "multiple in-progress entries: at most one allowed per plan".to_string(),
            entry: Some(c.name.clone()),
        })
        .collect()
}

fn check_project_in_registry(changes: &[Entry], registry: &Registry) -> Vec<Finding> {
    let project_names: HashSet<&str> = registry.projects.iter().map(|p| p.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        if let Some(project) = &entry.project
            && !project_names.contains(project.as_str())
        {
            out.push(Finding {
                level: Severity::Error,
                code: "project-not-in-registry",
                message: format!(
                    "project '{}' on change '{}' does not match any project in registry.yaml",
                    project, entry.name
                ),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

fn check_project_required_multi_repo(changes: &[Entry], registry: &Registry) -> Vec<Finding> {
    if registry.projects.len() <= 1 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in changes {
        if entry.project.is_none() && entry.capability.is_none() {
            out.push(Finding {
                level: Severity::Error,
                code: "project-missing-multi-repo",
                message: format!(
                    "change '{}' has no project or schema; multi-repo implementation changes must specify a project",
                    entry.name
                ),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

fn missing_project_or_schema(changes: &[Entry]) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in changes {
        if entry.project.is_none() && entry.capability.is_none() {
            out.push(Finding {
                level: Severity::Error,
                code: "plan.entry-needs-project-or-schema",
                message: format!(
                    "entry '{}' has neither 'project' nor 'schema'; at least one is required",
                    entry.name
                ),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

fn check_context_paths(changes: &[Entry]) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in changes {
        for path in &entry.context {
            if path.starts_with('/') || path.contains("..") {
                out.push(Finding {
                    level: Severity::Error,
                    code: "plan.context-path-invalid",
                    message: format!(
                        "entry '{}': context path '{}' must be relative to .specify/ (no '..' or absolute paths)",
                        entry.name, path
                    ),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

fn slices_dir_consistency(plan: &Plan, slices_dir: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let declared: HashSet<&str> = plan.entries.iter().map(|c| c.name.as_str()).collect();

    let Ok(read_dir) = std::fs::read_dir(slices_dir) else {
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
            out.push(Finding {
                level: Severity::Warning,
                code: "orphan-slice-dir",
                message: format!("slice directory '{name}' has no plan entry"),
                entry: Some(name.clone()),
            });
        }
    }

    for entry in &plan.entries {
        if entry.status == Status::InProgress {
            let candidate = slices_dir.join(&entry.name);
            if !candidate.is_dir() {
                out.push(Finding {
                    level: Severity::Warning,
                    code: "missing-slice-dir-for-in-progress",
                    message: format!(
                        "in-progress entry '{}' has no slice directory (may briefly be absent during phase start-up)",
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
mod tests;
