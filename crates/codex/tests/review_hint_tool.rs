//! Integration tests for the RFC-32 §D4 `tool` evaluator plus the
//! §D5 reserved-kind summary policy and the §"Acceptance" evidence
//! cap.
//!
//! The §D4 `tool` evaluator is exercised through a fake
//! [`specify_codex::review::eval::ToolRunner`] that simulates the
//! contract WASI tool's stdout. Wiring the real `specify-tool`
//! runtime would drag `wasmtime` into the standards crate's dep
//! graph; the CLI integration is S9's responsibility.

mod eval_support;

use std::fs;

use eval_support::{FakeToolRunner, NoToolRunner, hint, make_rule};
use specify_codex::review::ScanProfile;
use specify_codex::review::eval::{ReservedSkipped, ToolRunner, evaluate, reserved_hint_summary};
use specify_codex::review::index::build;
use specify_codex::rules::{FindingEvidence, HintKind, Severity, validate_evidence_size};

fn synthetic_envelope_stdout() -> Vec<u8> {
    let body = serde_json::json!({
        "version": 1,
        "summary": { "critical": 0, "important": 1, "suggestion": 0, "optional": 0 },
        "findings": [{
            "id": "FIND-0042",
            "rule-id": "CONTRACT-001",
            "title": "Contract drift detected",
            "severity": "important",
            "source": "deterministic",
            "artifact": "code",
            "evidence": {
                "kind": "snippet",
                "value": "GET /v1/foo removed without deprecation"
            },
            "impact": "downstream consumers break",
            "remediation": "Restore the endpoint or bump major.",
            "fingerprint": format!("sha256:{}", "ab".repeat(32))
        }]
    });
    serde_json::to_vec(&body).expect("serialise envelope")
}

#[test]
fn tool_envelope_findings_thread_through_unchanged() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("openapi.json"), "{}").expect("write openapi");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule("CONTRACT-001", vec![hint(HintKind::Tool, "contract")]);
    let runner: &dyn ToolRunner = &FakeToolRunner {
        declared: true,
        stdout: synthetic_envelope_stdout(),
        stderr: Vec::new(),
        exit_code: 0,
    };

    let outcome = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

    assert_eq!(outcome.findings.len(), 1, "exactly one finding from the synthetic envelope");
    let finding = &outcome.findings[0];
    assert_eq!(finding.rule_id.as_deref(), Some("CONTRACT-001"));
    assert_eq!(finding.title, "Contract drift detected");
    assert_eq!(finding.id, "FIND-0001", "umbrella restamps the id");
    assert!(finding.fingerprint.starts_with("sha256:"));
    assert!(matches!(finding.evidence, FindingEvidence::Snippet { .. }));
}

#[test]
fn tool_undeclared_emits_synthetic_finding() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("openapi.json"), "{}").expect("write openapi");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule("CONTRACT-002", vec![hint(HintKind::Tool, "contract")]);
    let runner: &dyn ToolRunner = &NoToolRunner;

    let outcome = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

    assert_eq!(outcome.findings.len(), 1);
    let finding = &outcome.findings[0];
    assert_eq!(finding.rule_id.as_deref(), Some("tool.undeclared"));
    assert_eq!(finding.severity, Severity::Important);
}

#[test]
fn tool_invocation_failed_truncates_oversize_stderr() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("openapi.json"), "{}").expect("write openapi");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule("CONTRACT-003", vec![hint(HintKind::Tool, "contract")]);
    let huge_stderr = vec![b'x'; 4 * 1024 * 1024];
    let runner: &dyn ToolRunner = &FakeToolRunner {
        declared: true,
        stdout: Vec::new(),
        stderr: huge_stderr,
        exit_code: 7,
    };

    let outcome = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

    assert_eq!(outcome.findings.len(), 1);
    let finding = &outcome.findings[0];
    assert_eq!(finding.rule_id.as_deref(), Some("tool.invocation-failed"));
    validate_evidence_size(finding).expect("evidence cap honoured after truncation");
    if let FindingEvidence::Snippet { value } = &finding.evidence {
        assert!(value.ends_with("…[truncated]"), "snippet must carry the truncation marker");
    } else {
        panic!("expected snippet evidence on tool.invocation-failed");
    }
    assert!(finding.fingerprint.starts_with("sha256:"));
}

#[test]
fn reserved_kind_collected_and_summary_minted() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("a.rs"), "fn x(){}\n").expect("write");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule("UNI-906", vec![hint(HintKind::Unique, "skill.name")]);
    let runner: &dyn ToolRunner = &NoToolRunner;

    let outcome = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

    assert!(outcome.findings.is_empty(), "reserved kinds emit no findings inline");
    assert_eq!(
        outcome.reserved_skipped,
        vec![ReservedSkipped {
            rule_id: "UNI-906".to_string(),
            hint_index: 0,
            kind: HintKind::Unique,
        }]
    );

    let optional = reserved_hint_summary(&outcome.reserved_skipped, false).expect("present");
    assert_eq!(optional.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
    assert_eq!(optional.severity, Severity::Optional);

    let strict = reserved_hint_summary(&outcome.reserved_skipped, true).expect("present");
    assert_eq!(strict.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
    assert_eq!(strict.severity, Severity::Important);

    if let FindingEvidence::Structured { data, .. } = &optional.evidence {
        let pairs = data.get("pairs").expect("pairs present");
        assert!(pairs.is_array(), "pairs must be an array");
    } else {
        panic!("reserved-kind summary must carry structured evidence");
    }
}
