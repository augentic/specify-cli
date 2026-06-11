use serde_json::{Value as JsonValue, json};

use super::{Report, Row, parsed_config, requested_rule, string_array_field, usize_field};

fn report_json(rows: Vec<Row<'_>>) -> JsonValue {
    serde_json::to_value(Report::from_rows(rows)).expect("serialise report")
}

#[test]
fn empty_report_shape() {
    let value = report_json(Vec::new());
    assert_eq!(value["version"], 1);
    assert_eq!(value["summary"], json!({ "critical": 0, "important": 0, "suggestion": 0, "optional": 0 }));
    assert_eq!(value["findings"], json!([]));
}

#[test]
fn finding_wire_shape() {
    // The envelope every host-side fold expects: kebab-case keys,
    // FIND-NNNN ids, snippet evidence, placeholder fingerprint, and an
    // omitted location for whole-tree findings.
    let rows = vec![
        Row {
            rule_id: "CORE-001",
            message: "first",
            path: Some("docs/a.md"),
            impact: "imp",
            remediation: "rem",
        },
        Row {
            rule_id: "CORE-002",
            message: "second",
            path: None,
            impact: "imp2",
            remediation: "rem2",
        },
    ];
    let value = report_json(rows);
    assert_eq!(value["summary"]["important"], 2);
    let first = &value["findings"][0];
    assert_eq!(first["id"], "FIND-0001");
    assert_eq!(first["rule-id"], "CORE-001");
    assert_eq!(first["title"], "first");
    assert_eq!(first["severity"], "important");
    assert_eq!(first["source"], "tool");
    assert_eq!(first["artifact"], "unknown");
    assert_eq!(first["location"]["path"], "docs/a.md");
    assert_eq!(first["evidence"], json!({ "kind": "snippet", "value": "first" }));
    assert_eq!(first["fingerprint"], super::PLACEHOLDER_FINGERPRINT);
    let second = &value["findings"][1];
    assert_eq!(second["id"], "FIND-0002");
    assert!(second.get("location").is_none(), "whole-tree finding omits location");
}

#[test]
fn requested_rule_scopes_by_sentinel() {
    let rules: &[&'static str] = &["CORE-028", "CORE-056"];
    let args = vec!["tool".to_string(), "adapters/shared/rules/core/CORE-056-x.md".to_string()];
    assert_eq!(requested_rule(&args, rules), Some("CORE-056"));
    let none = vec!["tool".to_string(), "README.md".to_string()];
    assert_eq!(requested_rule(&none, rules), None);
}

#[test]
fn config_helpers() {
    let args = vec![
        "tool".to_string(),
        "not-json".to_string(),
        r#"{"cap": 12, "names": ["a", "b"], "title": "t"}"#.to_string(),
    ];
    let config = parsed_config(&args);
    assert!(config.is_some());
    assert_eq!(usize_field(config.as_ref(), "cap"), 12);
    assert_eq!(usize_field(config.as_ref(), "missing"), 0);
    assert_eq!(string_array_field(config.as_ref(), "names"), vec!["a", "b"]);
    assert_eq!(super::string_field(config.as_ref(), "title"), "t");
    assert_eq!(super::string_field(config.as_ref(), "missing"), "");
}
