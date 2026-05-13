//! Stale workspace-clone detection for the `stale-workspace-clone`
//! diagnostic.

use std::path::Path;

use super::{CloneSignature, Diagnostic, DiagnosticPayload, STALE_CLONE, Severity, StaleReason};
use crate::registry::workspace::{SlotProblem, SlotProblemReason, slot_problem};
use crate::registry::{Registry, RegistryProject};

/// Stale-slot diagnostics for every project whose materialisation drifted.
///
/// Emits one [`STALE_CLONE`] per registry project whose existing
/// workspace slot would be refused by `workspace sync`. Missing slots
/// are left to `workspace sync`; absent `.specify-sync.yaml` metadata
/// is ignored.
pub(super) fn detect(registry: &Registry, project_dir: &Path) -> Vec<Diagnostic> {
    let mut sorted: Vec<&RegistryProject> = registry.projects.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::new();
    for project in sorted {
        if let Some(problem) = slot_problem(project_dir, project) {
            out.push(diag(project, &problem));
        }
    }
    out
}

fn diag(project: &RegistryProject, problem: &SlotProblem) -> Diagnostic {
    let expected = CloneSignature {
        slot_kind: Some(problem.expected_kind.to_string()),
        url: Some(project.url.clone()),
        capability: Some(project.capability.clone()),
        target: problem.expected_target.as_ref().map(|path| path.display().to_string()),
    };
    let observed = CloneSignature {
        slot_kind: problem.observed_kind.map(|kind| kind.to_string()),
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
        severity: Severity::Warning,
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
