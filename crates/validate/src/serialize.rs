//! JSON serialization for [`ValidationReport`].
//!
//! The shape is pinned to `schema_version: 1` per RFC-1 §"Output Format"
//! and is the source of truth for the goldens in `tests/fixtures/`. A
//! future `schema_version: 2` would be introduced via a new serializer
//! rather than mutating this one.

use serde_json::{Value, json};
use specify_schema::ValidationResult;

use crate::ValidationReport;

/// Serialise a [`ValidationReport`] to the canonical RFC-1 output shape.
///
/// The outermost object always carries `schema_version: 1`. Rule results
/// are emitted with a lowercase `status` string (`"pass"` / `"fail"` /
/// `"deferred"`) matching the RFC-1 examples.
pub fn serialize_report(report: &ValidationReport) -> Value {
    let mut brief_results = serde_json::Map::new();
    for (key, results) in &report.brief_results {
        let array: Vec<Value> = results.iter().map(validation_result_to_json).collect();
        brief_results.insert(key.clone(), Value::Array(array));
    }
    let cross_checks: Vec<Value> = report
        .cross_checks
        .iter()
        .map(validation_result_to_json)
        .collect();

    json!({
        "schema_version": 1,
        "passed": report.passed,
        "brief_results": Value::Object(brief_results),
        "cross_checks": Value::Array(cross_checks),
    })
}

fn validation_result_to_json(r: &ValidationResult) -> Value {
    match r {
        ValidationResult::Pass { rule_id, rule } => json!({
            "status": "pass",
            "rule_id": rule_id,
            "rule": rule,
        }),
        ValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => json!({
            "status": "fail",
            "rule_id": rule_id,
            "rule": rule,
            "detail": detail,
        }),
        ValidationResult::Deferred {
            rule_id,
            rule,
            reason,
        } => json!({
            "status": "deferred",
            "rule_id": rule_id,
            "rule": rule,
            "reason": reason,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn serializes_passed_report_with_schema_version() {
        let mut brief_results: BTreeMap<String, Vec<ValidationResult>> = BTreeMap::new();
        brief_results.insert(
            "proposal".to_string(),
            vec![ValidationResult::Pass {
                rule_id: "proposal.why-has-content",
                rule: "Has a Why section with at least one sentence",
            }],
        );
        let report = ValidationReport {
            brief_results,
            cross_checks: vec![],
            passed: true,
        };
        let value = serialize_report(&report);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["passed"], true);
        assert_eq!(value["brief_results"]["proposal"][0]["status"], "pass");
    }

    #[test]
    fn serializes_deferred_with_reason() {
        let report = ValidationReport {
            brief_results: BTreeMap::new(),
            cross_checks: vec![ValidationResult::Deferred {
                rule_id: "specs.uses-normative-language",
                rule: "Uses SHALL/MUST language for normative requirements",
                reason: "Semantic check — requires LLM judgment",
            }],
            passed: true,
        };
        let value = serialize_report(&report);
        assert_eq!(value["cross_checks"][0]["status"], "deferred");
        assert_eq!(
            value["cross_checks"][0]["reason"],
            "Semantic check — requires LLM judgment"
        );
    }

    #[test]
    fn serializes_failure_with_detail() {
        let report = ValidationReport {
            brief_results: BTreeMap::new(),
            cross_checks: vec![ValidationResult::Fail {
                rule_id: "cross.design-references-valid",
                rule: "Every requirement id referenced in design.md exists in specs",
                detail: "REQ-999 not found".to_string(),
            }],
            passed: false,
        };
        let value = serialize_report(&report);
        assert_eq!(value["passed"], false);
        assert_eq!(value["cross_checks"][0]["status"], "fail");
        assert_eq!(value["cross_checks"][0]["detail"], "REQ-999 not found");
    }
}
