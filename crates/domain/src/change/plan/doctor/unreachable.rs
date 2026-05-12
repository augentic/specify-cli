//! Unreachable-entry detection for the `unreachable-entry` diagnostic.

use std::collections::{HashMap, HashSet};

use super::{
    BlockingPredecessor, Diagnostic, DiagnosticPayload, DiagnosticSeverity, UNREACHABLE, cycle,
};
use crate::change::plan::core::{Entry, Status};

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
pub(super) fn detect(changes: &[Entry]) -> Vec<Diagnostic> {
    let cycles = cycle::membership(changes);

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
