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

fn report_with_outputs(status: &str, outputs: &[Value]) -> BuildReport {
    serde_json::from_value(json!({
        "version": 1,
        "slice": "identity-service",
        "target": "vectis@v1",
        "status": status,
        "findings": [],
        "outputs": outputs,
    }))
    .expect("report with outputs deserialises")
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
fn report_without_outputs_round_trips() {
    let report = report("success", &[]);
    assert!(report.outputs.is_empty(), "missing outputs defaults to empty");
    let serialised = serde_json::to_string(&report).expect("serialise");
    assert!(!serialised.contains("outputs"), "empty outputs is skipped in serialisation");
    let reparsed: BuildReport = serde_json::from_str(&serialised).expect("re-deserialise");
    assert_eq!(report, reparsed);
}

#[test]
fn report_with_outputs_round_trips() {
    let report = report_with_outputs(
        "success",
        &[
            json!({ "platform": "core", "path": "shared/src/app.rs" }),
            json!({ "platform": "ios", "path": "iOS/MyApp/ContentView.swift" }),
        ],
    );
    assert_eq!(report.outputs.len(), 2);
    assert_eq!(report.outputs[0].platform, Platform::Core);
    assert_eq!(report.outputs[0].path, "shared/src/app.rs");
    assert_eq!(report.outputs[1].platform, Platform::Ios);

    let serialised = serde_json::to_string(&report).expect("serialise");
    let reparsed: BuildReport = serde_json::from_str(&serialised).expect("re-deserialise");
    assert_eq!(report, reparsed);
}

#[test]
fn gate_success_blocks_finding() {
    let report = report("success", &[finding("critical")]);
    match enforce_report_no_blocking_on_success(&report) {
        Err(Error::Validation { code, .. }) => {
            assert_eq!(code, "target-build-success-with-blocking-finding");
        }
        other => panic!("expected blocking-finding gate to fire, got {other:?}"),
    }
}

#[test]
fn gate_success_non_blocking_ok() {
    let report = report("success", &[finding("suggestion")]);
    enforce_report_no_blocking_on_success(&report).expect("non-blocking success passes");
}

#[test]
fn gate_failure_blocking_ok() {
    let report = report("failure", &[finding("critical")]);
    enforce_report_no_blocking_on_success(&report).expect("failure may carry blocking findings");
}

#[test]
fn output_gate_accepts_empty_outputs() {
    let report = report("success", &[]);
    let dir = tempfile::tempdir().expect("tempdir");
    enforce_report_outputs_exist(&report, dir.path()).expect("empty outputs passes");
}

#[test]
fn output_gate_accepts_present_outputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("shared/src")).expect("mkdir");
    std::fs::write(dir.path().join("shared/src/app.rs"), "fn main() {}").expect("write");

    let report = report_with_outputs(
        "success",
        &[json!({ "platform": "core", "path": "shared/src/app.rs" })],
    );
    enforce_report_outputs_exist(&report, dir.path()).expect("present output passes");
}

#[test]
fn output_gate_rejects_missing_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = report_with_outputs(
        "success",
        &[json!({ "platform": "ios", "path": "iOS/MyApp/ContentView.swift" })],
    );
    match enforce_report_outputs_exist(&report, dir.path()) {
        Err(Error::Validation { code, .. }) => {
            assert_eq!(code, "target-build-output-missing");
        }
        other => panic!("expected output-missing gate, got {other:?}"),
    }
}

#[test]
fn output_gate_rejects_empty_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("shared/src")).expect("mkdir");
    std::fs::write(dir.path().join("shared/src/app.rs"), "").expect("write empty");

    let report = report_with_outputs(
        "success",
        &[json!({ "platform": "core", "path": "shared/src/app.rs" })],
    );
    match enforce_report_outputs_exist(&report, dir.path()) {
        Err(Error::Validation { code, .. }) => {
            assert_eq!(code, "target-build-output-missing");
        }
        other => panic!("expected output-missing gate for empty file, got {other:?}"),
    }
}

#[test]
fn output_gate_skips_on_failure_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = report_with_outputs(
        "failure",
        &[json!({ "platform": "ios", "path": "iOS/MyApp/ContentView.swift" })],
    );
    enforce_report_outputs_exist(&report, dir.path()).expect("failure status skips output check");
}

#[test]
fn output_gate_accepts_tree_output() {
    // Targets like vectis declare per-platform tree paths (`shared/`).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("shared/src")).expect("mkdir");
    std::fs::write(dir.path().join("shared/src/app.rs"), "fn main() {}").expect("write");

    let report =
        report_with_outputs("success", &[json!({ "platform": "core", "path": "shared/" })]);
    enforce_report_outputs_exist(&report, dir.path()).expect("non-empty tree output passes");
}

#[test]
fn output_gate_rejects_empty_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("shared")).expect("mkdir");

    let report =
        report_with_outputs("success", &[json!({ "platform": "core", "path": "shared" })]);
    match enforce_report_outputs_exist(&report, dir.path()) {
        Err(Error::Validation { code, detail }) => {
            assert_eq!(code, "target-build-output-missing");
            assert!(detail.contains("exists but is empty"), "detail: {detail}");
        }
        other => panic!("expected output-missing gate for empty directory, got {other:?}"),
    }
}

#[test]
fn output_gate_rejects_absolute_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report =
        report_with_outputs("success", &[json!({ "platform": "core", "path": "/etc/passwd" })]);
    match enforce_report_outputs_exist(&report, dir.path()) {
        Err(Error::Validation { code, detail }) => {
            assert_eq!(code, "target-build-output-missing");
            assert!(detail.contains("absolute or contains `..`"), "detail: {detail}");
        }
        other => panic!("expected output-missing gate for absolute path, got {other:?}"),
    }
}

#[test]
fn output_gate_rejects_parent_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("secret.txt"), "secret").expect("write");

    let report =
        report_with_outputs("success", &[json!({ "platform": "core", "path": "../secret.txt" })]);
    match enforce_report_outputs_exist(&report, dir.path()) {
        Err(Error::Validation { code, detail }) => {
            assert_eq!(code, "target-build-output-missing");
            assert!(detail.contains("absolute or contains `..`"), "detail: {detail}");
        }
        other => panic!("expected output-missing gate for traversal path, got {other:?}"),
    }
}
