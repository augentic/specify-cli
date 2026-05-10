//! Orphan source-key detection for the `orphan-source-key` diagnostic.

use std::collections::HashSet;

use super::{Diagnostic, DiagnosticPayload, DiagnosticSeverity, ORPHAN_SOURCE};
use crate::plan::core::Plan;

/// Top-level `sources:` keys declared but not referenced by any entry.
///
/// The inverse of validate's `unknown-source`, which catches *entry
/// references* to undeclared keys; this catches *declarations* with no
/// references.
pub(super) fn detect(plan: &Plan) -> Vec<Diagnostic> {
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
