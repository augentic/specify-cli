//! Integration test for the the executable hint-kind contract
//! `regex` evaluator.
//!
//! Verifies the the file scan contract binary-skip rule (a NUL-byte file is ignored
//! even when its path passes the path-pattern filter) and the
//! per-finding location / snippet shape.

mod eval_support;

use std::fs;

use eval_support::{NoToolRunner, hint, make_rule};
use specify_diagnostics::FindingEvidence;
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::HintKind;

#[test]
fn matches_text_skips_binaries() {
    let tmp = tempfile::tempdir().expect("tmp");
    let rs_body = "use std::env;\nfn endpoint() -> &'static str { \"https://api.example.com\" }\n";
    fs::write(tmp.path().join("app.rs"), rs_body).expect("write app.rs");

    fs::create_dir_all(tmp.path().join(".specify")).expect("mk .specify");
    fs::write(tmp.path().join(".specify/blob.bin"), b"\x00\x00binary contents https://hidden\n")
        .expect("write blob");

    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule("UNI-014", vec![hint(HintKind::Regex, "https?://")]);
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

    assert_eq!(outcome.findings.len(), 1, "binary file must be skipped");
    let finding = &outcome.findings[0];
    let location = finding.location.as_ref().expect("location set");
    assert_eq!(location.path, "app.rs");
    assert_eq!(location.line, Some(2), "URL is on the second line");
    assert!(location.column.expect("column set") >= 1);
    match &finding.evidence {
        FindingEvidence::Snippet { value } => {
            assert!(
                value.contains("https://api.example.com"),
                "snippet must carry the matched line: {value}"
            );
        }
        other => panic!("expected snippet evidence, got {other:?}"),
    }
    assert_eq!(finding.id, "FIND-0001");
    assert!(finding.fingerprint.starts_with("sha256:"));
}

#[test]
fn regex_compile_failure_is_hard_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("a.rs"), "let x = 1;\n").expect("write");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule("UNI-903", vec![hint(HintKind::Regex, "(unclosed")]);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let err = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect_err("regex must fail to compile");
    assert!(format!("{err}").contains("regex"), "error must mention regex: {err}");
}
