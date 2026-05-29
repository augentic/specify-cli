//! `kind: set-eq` evaluator per RFC-34 §F6 (C15).
//!
//! Asserts that the set of values some candidate file declares
//! EXACTLY EQUALS a closed expected set — the two-sided tightening of
//! C12's one-sided [`crate::lint::eval::set_coverage`]. v1 supports
//! one source discriminator — `adapter-briefs-equal-operations` —
//! which consumes the [`crate::lint::AdapterManifest`] facts the
//! framework-profile indexer already produced
//! (see [`crate::lint::index::adapter::extract`]) and flags each
//! `adapters/{sources,targets}/<name>/adapter.yaml` whose
//! `briefs.keys()` set is not exactly the axis-appropriate closed
//! operation enum (`SourceOperation::{Extract, Survey}` xor
//! `TargetOperation::{Shape, Build, Merge}`). The interpreter emits
//! one [`crate::rules::Diagnostic`] per `(adapter, divergence)`
//! pair, where the divergence is either a `missing` operation (in the
//! expected enum, absent from `briefs.keys()`) or an `unexpected` key
//! (present in `briefs.keys()`, absent from the expected enum). The
//! manifest path is the finding's location and the per-adapter
//! `(divergence, operation, expected, actual)` shape is surfaced via
//! [`crate::rules::FindingEvidence::Structured`] for downstream
//! tooling.
//!
//! `set-eq` fires on both halves where `set-coverage` is silent on
//! extras: the `missing` half overlaps `set-coverage` (and the
//! per-axis schema's `required` list), and the `unexpected` half
//! overlaps the per-axis schema's `additionalProperties: false`
//! rejection of unknown keys. The overlap is intentional; the
//! fingerprint algorithm dedupes identical `(rule-id, location,
//! evidence)` triples and distinct rule ids land side-by-side.
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

const SOURCE_ADAPTER_BRIEFS_EQUAL_OPERATIONS: &str = "adapter-briefs-equal-operations";

/// Divergence direction for an operation that breaks set equality.
const DIVERGENCE_MISSING: &str = "missing";
const DIVERGENCE_UNEXPECTED: &str = "unexpected";

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
    if source != SOURCE_ADAPTER_BRIEFS_EQUAL_OPERATIONS {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetEq,
            reason: "only `adapter-briefs-equal-operations` is supported in v1",
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

        // Two-sided diff: expected\actual are `missing`, actual\expected
        // are `unexpected`. Collected as a sorted `(divergence, op)` list
        // so finding order is deterministic across runs.
        let mut divergences: Vec<(&'static str, String)> = Vec::new();
        for op in expected.iter().copied() {
            if !actual.contains(op) {
                divergences.push((DIVERGENCE_MISSING, op.to_owned()));
            }
        }
        for key in actual.iter().copied() {
            if !expected.contains(key) {
                divergences.push((DIVERGENCE_UNEXPECTED, key.to_owned()));
            }
        }
        if divergences.is_empty() {
            continue;
        }
        divergences.sort_unstable();

        let expected_sorted: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        let actual_sorted: Vec<String> = actual.iter().map(|s| (*s).to_string()).collect();
        for (divergence, op) in divergences {
            let location = FindingLocation {
                path: manifest.path.clone(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            };
            let evidence = FindingEvidence::Structured {
                summary: format!(
                    "adapter '{}' brief set diverges: {} operation '{}'",
                    manifest.name, divergence, op,
                ),
                data: serde_json::json!({
                    "adapter": manifest.name,
                    "axis": axis_token(manifest.axis),
                    "divergence": divergence,
                    "operation": op,
                    "expected": expected_sorted,
                    "actual": actual_sorted,
                }),
                locations: None,
            };
            let title = format!(
                "{}: adapter '{}' has {} brief operation '{}'",
                rule.title, manifest.name, divergence, op,
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
