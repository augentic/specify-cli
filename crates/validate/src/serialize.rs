//! JSON serialization for [`ValidationReport`].
//!
//! Emits the report payload only; the surrounding `schema-version`
//! envelope is added by the CLI's `emit_response`.

use serde_json::{Value, json};
use specify_capability::ValidationResult;

use crate::ValidationReport;

/// Serialise a [`ValidationReport`] as the canonical kebab-case payload.
pub fn serialize_report(report: &ValidationReport) -> Value {
    let mut brief_results = serde_json::Map::new();
    for (key, results) in &report.brief_results {
        let array: Vec<Value> = results.iter().map(validation_result_to_json).collect();
        brief_results.insert(key.clone(), Value::Array(array));
    }
    let cross_checks: Vec<Value> =
        report.cross_checks.iter().map(validation_result_to_json).collect();

    json!({
        "passed": report.passed,
        "brief-results": Value::Object(brief_results),
        "cross-checks": Value::Array(cross_checks),
    })
}

fn validation_result_to_json(r: &ValidationResult) -> Value {
    match r {
        ValidationResult::Pass { rule_id, rule } => json!({
            "status": "pass",
            "rule-id": rule_id,
            "rule": rule,
        }),
        ValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => json!({
            "status": "fail",
            "rule-id": rule_id,
            "rule": rule,
            "detail": detail,
        }),
        ValidationResult::Deferred {
            rule_id,
            rule,
            reason,
        } => json!({
            "status": "deferred",
            "rule-id": rule_id,
            "rule": rule,
            "reason": reason,
        }),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn pass_report_payload() {
        let mut brief_results: BTreeMap<String, Vec<ValidationResult>> = BTreeMap::new();
        brief_results.insert(
            "proposal".to_string(),
            vec![ValidationResult::Pass {
                rule_id: "proposal.why-has-content".into(),
                rule: "Has a Why section with at least one sentence".into(),
            }],
        );
        let report = ValidationReport {
            brief_results,
            cross_checks: vec![],
            passed: true,
        };
        let value = serialize_report(&report);
        assert_eq!(value["passed"], true);
        assert_eq!(value["brief-results"]["proposal"][0]["status"], "pass");
    }

    #[test]
    fn deferred_includes_reason() {
        let report = ValidationReport {
            brief_results: BTreeMap::new(),
            cross_checks: vec![ValidationResult::Deferred {
                rule_id: "specs.uses-normative-language".into(),
                rule: "Uses SHALL/MUST language for normative requirements".into(),
                reason: "Semantic check — requires LLM judgment",
            }],
            passed: true,
        };
        let value = serialize_report(&report);
        assert_eq!(value["cross-checks"][0]["status"], "deferred");
        assert_eq!(value["cross-checks"][0]["reason"], "Semantic check — requires LLM judgment");
    }

    #[test]
    fn fail_includes_detail() {
        let report = ValidationReport {
            brief_results: BTreeMap::new(),
            cross_checks: vec![ValidationResult::Fail {
                rule_id: "cross.design-references-valid".into(),
                rule: "Every requirement id referenced in design.md exists in specs".into(),
                detail: "REQ-999 not found".to_string(),
            }],
            passed: false,
        };
        let value = serialize_report(&report);
        assert_eq!(value["passed"], false);
        assert_eq!(value["cross-checks"][0]["status"], "fail");
        assert_eq!(value["cross-checks"][0]["detail"], "REQ-999 not found");
    }
}
