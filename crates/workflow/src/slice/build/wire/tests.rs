use serde_json::{Value, json};

use super::*;

/// A minimal schema-valid [`Diagnostic`] JSON of the given severity,
/// left at the default `violation` kind and untriaged status so
/// `critical` / `important` instances block.
fn finding(severity: &str) -> Value {
    json!({
        "id": "DIAG-0001",
        "title": "test finding",
        "severity": severity,
        "source": "tool",
        "artifact": "code",
        "evidence": { "kind": "snippet", "value": "x" },
        "impact": "impact",
        "remediation": "fix it",
        "fingerprint": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    })
}

fn report(status: &str, findings: &[Value]) -> BuildReport {
    serde_json::from_value(json!({
        "version": 1,
        "slice": "identity-service",
        "target": "omnia@v1",
        "status": status,
        "findings": findings,
    }))
    .expect("report deserialises")
}

#[test]
fn request_round_trips() {
    let req = json!({
        "version": 1,
        "slice": "identity-service",
        "project-dir": "/w/.specify/workspace/identity-service",
        "inputs": {
            "root": "/w/.specify/slices/identity-service",
            "artifacts": {
                "proposal": "proposal.md",
                "design": "design.md",
                "tasks": "tasks.md",
                "specs": ["specs/identity/spec.md"],
                "additional": ["tokens.yaml"]
            }
        }
    });
    let parsed: BuildRequest = serde_json::from_value(req).expect("request deserialises");
    assert_eq!(parsed.version, BUILD_VERSION);
    assert_eq!(parsed.slice, "identity-service");
    assert_eq!(parsed.inputs.artifacts.specs, vec!["specs/identity/spec.md".to_string()]);
    assert_eq!(parsed.inputs.artifacts.additional, vec!["tokens.yaml".to_string()]);

    let serialised = serde_json::to_string(&parsed).expect("serialise request");
    assert!(serialised.contains("project-dir"), "project-dir renders kebab-case");
    let reparsed: BuildRequest = serde_json::from_str(&serialised).expect("re-deserialise");
    assert_eq!(parsed, reparsed);
}

#[test]
fn report_rejects_unknown_field() {
    let bogus = json!({
        "version": 1,
        "slice": "identity-service",
        "target": "omnia@v1",
        "status": "success",
        "findings": [],
        "stray": true
    });
    serde_json::from_value::<BuildReport>(bogus)
        .expect_err("deny_unknown_fields rejects stray keys");
}

#[test]
fn gate_rejects_success_with_blocking_finding() {
    let report = report("success", &[finding("critical")]);
    match enforce_report_no_blocking_on_success(&report) {
        Err(Error::Validation { code, .. }) => {
            assert_eq!(code, "target-build-success-with-blocking-finding");
        }
        other => panic!("expected blocking-finding gate to fire, got {other:?}"),
    }
}

#[test]
fn gate_accepts_success_with_only_non_blocking_findings() {
    let report = report("success", &[finding("suggestion")]);
    enforce_report_no_blocking_on_success(&report).expect("non-blocking success passes");
}

#[test]
fn gate_accepts_failure_with_blocking_finding() {
    let report = report("failure", &[finding("critical")]);
    enforce_report_no_blocking_on_success(&report).expect("failure may carry blocking findings");
}
