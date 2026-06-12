//! `kind: set-eq` evaluator.
//!
//! Asserts that the set of values some candidate file declares
//! EXACTLY EQUALS a closed expected set — the two-sided tightening of
//! C12's one-sided [`crate::lint::eval::set_coverage`]. v1 supports
//! one source discriminator — `adapter-briefs` —
//! which consumes the [`crate::lint::AdapterManifest`] facts the
//! framework-profile indexer already produced
//! (see [`crate::lint::index::adapter::extract`]) and flags each
//! `adapters/{sources,targets}/<name>/adapter.yaml` whose
//! `briefs.keys()` set is not exactly the axis-appropriate operation
//! set the rule supplies in `config: { expected-operations }` — **policy
//! supplied by the rule file**, never a `const` in this arm (per the
//! standards-layer policy-in-`specify` rule). The interpreter emits
//! one [`specify_diagnostics::Diagnostic`] per `(adapter, divergence)`
//! pair, where the divergence is either a `missing` operation (in the
//! expected enum, absent from `briefs.keys()`) or an `unexpected` key
//! (present in `briefs.keys()`, absent from the expected enum). The
//! manifest path is the finding's location and the per-adapter
//! `(divergence, operation, expected, actual)` shape is surfaced via
//! [`specify_diagnostics::FindingEvidence::Structured`] for downstream
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

use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::lint::adapter_briefs::{ExpectedOperationsConfig, axis_token};
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_ADAPTER_BRIEFS: &str = "adapter-briefs";

/// Divergence direction for an operation that breaks set equality.
const DIVERGENCE_MISSING: &str = "missing";
const DIVERGENCE_UNEXPECTED: &str = "unexpected";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_ADAPTER_BRIEFS {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetEq,
            reason: "only `adapter-briefs` is supported in v1",
        });
    }
    let cfg = ExpectedOperationsConfig::parse(hint.config.as_ref()).ok_or_else(|| {
        HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetEq,
            reason: "`adapter-briefs` requires a `config: { expected-operations }`",
        }
    })?;

    let candidate_set = super::candidate_set(candidates);

    let mut out: Vec<Diagnostic> = Vec::new();
    for manifest in &model.adapter_manifests {
        if !candidate_set.contains(&manifest.path) {
            continue;
        }
        let expected = cfg.expected_for(manifest.axis);
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

#[cfg(test)]
mod unit {
    use serde_json::json;

    use super::*;
    use crate::lint::eval::testkit::{candidates, empty_model, hint, hint_with_config, rule};
    use crate::lint::{AdapterAxis, AdapterManifest};

    fn manifest(name: &str, axis: AdapterAxis, briefs: &[&str]) -> AdapterManifest {
        let axis_dir = match axis {
            AdapterAxis::Sources => "sources",
            AdapterAxis::Targets => "targets",
        };
        AdapterManifest {
            axis,
            name: name.to_string(),
            path: format!("adapters/{axis_dir}/{name}/adapter.yaml"),
            version: None,
            brief_keys: briefs.iter().map(|b| (*b).to_string()).collect(),
            tools: vec![],
        }
    }

    // Fixture operation names are deliberately not the real adapter
    // operations: the expected sets are rule-supplied policy, and the
    // `no_embedded_policy` guard rejects real operation-set literals
    // anywhere under `lint/eval/`.
    fn expected_ops() -> serde_json::Value {
        json!({ "expected-operations": { "sources": ["alpha", "beta"], "targets": ["gamma", "delta", "epsilon"] } })
    }

    #[test]
    fn missing_and_unexpected_both_flagged() {
        let mut model = empty_model();
        model.adapter_manifests =
            vec![manifest("demo", AdapterAxis::Targets, &["gamma", "delta", "rogue"])];
        let cands = candidates(&["adapters/targets/demo/adapter.yaml"]);
        let hint = hint_with_config(HintKind::SetEq, "adapter-briefs", Some(expected_ops()));
        let out = evaluate(&rule(), &hint, &cands, &model, &mut 1).expect("evaluate");
        let titles: Vec<&str> = out.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(out.len(), 2, "{titles:?}");
        assert!(
            titles.iter().any(|t| t.contains("missing brief operation 'epsilon'")),
            "{titles:?}"
        );
        assert!(
            titles.iter().any(|t| t.contains("unexpected brief operation 'rogue'")),
            "{titles:?}"
        );
    }

    #[test]
    fn exact_set_is_silent() {
        let mut model = empty_model();
        model.adapter_manifests = vec![manifest("demo", AdapterAxis::Sources, &["alpha", "beta"])];
        let cands = candidates(&["adapters/sources/demo/adapter.yaml"]);
        let hint = hint_with_config(HintKind::SetEq, "adapter-briefs", Some(expected_ops()));
        let out = evaluate(&rule(), &hint, &cands, &model, &mut 1).expect("evaluate");
        assert!(out.is_empty());
    }

    #[test]
    fn manifest_outside_candidates_skipped() {
        let mut model = empty_model();
        model.adapter_manifests = vec![manifest("demo", AdapterAxis::Targets, &["gamma"])];
        let hint = hint_with_config(HintKind::SetEq, "adapter-briefs", Some(expected_ops()));
        let out = evaluate(&rule(), &hint, &[], &model, &mut 1).expect("evaluate");
        assert!(out.is_empty());
    }

    #[test]
    fn missing_config_is_unsupported() {
        let model = empty_model();
        let hint = hint(HintKind::SetEq, "adapter-briefs");
        evaluate(&rule(), &hint, &[], &model, &mut 1).unwrap_err();
    }

    #[test]
    fn unknown_source_is_unsupported() {
        let model = empty_model();
        let hint = hint_with_config(HintKind::SetEq, "no-such-source", Some(expected_ops()));
        evaluate(&rule(), &hint, &[], &model, &mut 1).unwrap_err();
    }
}
