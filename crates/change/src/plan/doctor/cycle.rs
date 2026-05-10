//! Cycle detection for the `cycle-in-depends-on` diagnostic.

use std::collections::{HashMap, HashSet};

use petgraph::algo::tarjan_scc;
use petgraph::graph::DiGraph;

use super::{CYCLE, Diagnostic, DiagnosticPayload, DiagnosticSeverity};
use crate::plan::core::Entry;

/// One [`CYCLE`] diagnostic per cycle in the depends-on graph.
///
/// Self-loops are emitted too. Cycles are deduplicated by sorted
/// node-set so every distinct cycle surfaces exactly once. The cycle
/// path is sorted alphabetically with the first node repeated at the
/// end — matches the convention used by validate's `dependency-cycle`
/// text.
pub(super) fn detect(changes: &[Entry]) -> Vec<Diagnostic> {
    let (graph, _) = build_graph(changes);

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

/// Return the set of entry names that participate in any cycle.
///
/// Self-loops are included. Used by the unreachable check to avoid
/// double-reporting entries that are already surfaced under [`CYCLE`].
pub(super) fn membership(changes: &[Entry]) -> HashSet<&str> {
    let (graph, _) = build_graph(changes);

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

fn build_graph(
    changes: &[Entry],
) -> (DiGraph<&str, ()>, HashMap<&str, petgraph::graph::NodeIndex>) {
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
    (graph, idx)
}
