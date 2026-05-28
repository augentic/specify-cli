//! Integration tests for the `kind: tool` evaluator contract `tool` evaluator plus the
//! reserved-hint diagnostics reserved-kind summary policy and the §"Acceptance" evidence
//! cap.
//!
//! The `kind: tool` evaluator contract `tool` evaluator is exercised through a fake
//! [`specify_lints::lint::eval::ToolRunner`] that simulates the
//! contract WASI tool's stdout. Wiring the real `specify-tool`
//! runtime would drag `wasmtime` into the standards crate's dep
//! graph; the CLI integration is S9's responsibility.

mod eval_support;

use std::fs;

use eval_support::{FakeToolRunner, NoToolRunner, hint, make_rule};
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ReservedSkipped, ToolRunner, evaluate, reserved_hint_summary};
use specify_lints::lint::index::build;
use specify_lints::rules::{FindingEvidence, HintKind, Severity, validate_evidence_size};

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

// After C17 no hint kind is reserved, so `evaluate` can no longer
// populate `reserved_skipped`. The reserved-kind machinery survives as
// forward-compat scaffolding for any future kind landed reserved; this
// test exercises the summary-minting fold directly by constructing a
// `ReservedSkipped` value (its `kind` is just a `HintKind` field, so
// any variant works as a sample) and asserting `reserved_hint_summary`
// still mints the `review.reserved-hint-skipped` finding in both modes.
#[test]
fn reserved_summary_folds_skipped_entries_in_both_modes() {
    let skipped = vec![ReservedSkipped {
        rule_id: "UNI-906".to_string(),
        hint_index: 0,
        kind: HintKind::NamespaceOwner,
    }];

    let optional = reserved_hint_summary(&skipped, false).expect("present");
    assert_eq!(optional.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
    assert_eq!(optional.severity, Severity::Optional);

    let strict = reserved_hint_summary(&skipped, true).expect("present");
    assert_eq!(strict.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
    assert_eq!(strict.severity, Severity::Important);

    if let FindingEvidence::Structured { data, .. } = &optional.evidence {
        let pairs = data.get("pairs").expect("pairs present");
        assert!(pairs.is_array(), "pairs must be an array");
    } else {
        panic!("reserved-kind summary must carry structured evidence");
    }
}
