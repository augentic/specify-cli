//! `kind: set-coverage` evaluator.
//!
//! Asserts that the set of values some candidate file declares
//! covers a closed expected set. v1 supports one source
//! discriminator — `adapter-briefs-cover-operations` — which
//! consumes the [`crate::lint::AdapterManifest`] facts the
//! framework-profile indexer already produced
//! (see [`crate::lint::index::adapter::extract`]) and flags each
//! `adapters/{sources,targets}/<name>/adapter.yaml` whose
//! `briefs.keys()` set is missing one or more operations from the
//! axis-appropriate closed enum
//! (`SourceOperation::{Extract, Survey}` xor
//! `TargetOperation::{Shape, Build, Merge}`). The interpreter emits
//! one [`crate::rules::Diagnostic`] per `(adapter, missing-operation)`
//! pair, with the manifest path as the finding's location and the
//! per-adapter `(missing, expected, actual)` triple surfaced via
//! [`crate::rules::FindingEvidence::Structured`] for downstream
//! tooling.
//!
//! `set-coverage` is one-sided by design: extras (`briefs.keys()`
//! values not in the expected operation set) are silent at this
//! layer. A future `kind: set-eq` evaluator
//! tightens the contract to require strict set equality.
//!
//! Adapter manifests whose `path` is not in the caller-supplied
//! candidate set are ignored, so the closed `path-pattern` filter
//! the umbrella evaluator builds still drives candidate selection.
//! Manifests the indexer drops upstream (binary `adapter.yaml`,
//! YAML body without a non-empty `name:` value, etc.) never reach
//! this layer.
//!
//! Future hint values may extend the closed source set; unknown
//! discriminators are rejected as [`super::HintError::Unsupported`]
//! so authoring drift surfaces at hint-evaluation time rather than
//! silently passing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::{HintError, make_finding};
use crate::lint::{AdapterAxis, WorkspaceModel};
use crate::rules::{
    DeterministicHint, Diagnostic, FindingEvidence, FindingLocation, HintKind, ResolvedRule,
};

const SOURCE_ADAPTER_BRIEFS_COVER_OPERATIONS: &str = "adapter-briefs-cover-operations";

/// Closed source-adapter operation set kept in sync with
/// `specify_workflow::adapter::SourceOperation` (kebab-case wire
/// form). Held inline here so the standards-layer crate does not
/// take a workflow-layer dependency.
const SOURCE_OPERATIONS: &[&str] = &["extract", "survey"];

/// Closed target-adapter operation set kept in sync with
/// `specify_workflow::adapter::TargetOperation`.
const TARGET_OPERATIONS: &[&str] = &["build", "merge", "shape"];

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_ADAPTER_BRIEFS_COVER_OPERATIONS {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetCoverage,
            reason: "only `adapter-briefs-cover-operations` is supported in v1",
        });
    }

    let candidate_set: BTreeSet<String> =
        candidates.iter().map(|p| p.to_string_lossy().into_owned()).collect();

    let mut out: Vec<Diagnostic> = Vec::new();
    for manifest in &model.adapter_manifests {
        if !candidate_set.contains(&manifest.path) {
            continue;
        }
        let expected: BTreeSet<&'static str> = match manifest.axis {
            AdapterAxis::Sources => SOURCE_OPERATIONS.iter().copied().collect(),
            AdapterAxis::Targets => TARGET_OPERATIONS.iter().copied().collect(),
        };
        let actual: BTreeSet<&str> = manifest.brief_keys.iter().map(String::as_str).collect();
        let mut missing: Vec<&str> =
            expected.iter().copied().filter(|op| !actual.contains(op)).collect();
        if missing.is_empty() {
            continue;
        }
        missing.sort_unstable();
        let expected_sorted: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        let actual_sorted: Vec<String> = actual.iter().map(|s| (*s).to_string()).collect();
        for op in missing {
            let location = FindingLocation {
                path: manifest.path.clone(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            };
            let evidence = FindingEvidence::Structured {
                summary: format!(
                    "adapter '{}' is missing brief for operation '{}'",
                    manifest.name, op,
                ),
                data: serde_json::json!({
                    "adapter": manifest.name,
                    "axis": axis_token(manifest.axis),
                    "missing": op,
                    "expected": expected_sorted,
                    "actual": actual_sorted,
                }),
                locations: None,
            };
            let title = format!(
                "{}: adapter '{}' missing brief for operation '{}'",
                rule.title, manifest.name, op,
            );
            let finding = make_finding(rule, *next_id, title, Some(location), evidence);
            *next_id += 1;
            out.push(finding);
        }
    }
    Ok(out)
}

const fn axis_token(axis: AdapterAxis) -> &'static str {
    match axis {
        AdapterAxis::Sources => "sources",
        AdapterAxis::Targets => "targets",
    }
}
