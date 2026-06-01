//! Integration test for the the executable hint-kind contract
//! `path-pattern` evaluator.
//!
//! `path-pattern` is a candidate-set filter, not a finder. The lint contract
//! contract is verified indirectly: a rule with `path-pattern: *.rs`
//! plus `regex: fn` MUST emit findings only for the `*.rs` files
//! reachable from the workspace model, and the reserved-hint diagnostics reserved-skipped
//! list MUST stay empty.

mod eval_support;

use std::fs;

use eval_support::{NoToolRunner, hint, make_rule};
use specify_diagnostics::Severity;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

#[test]
fn path_pattern_narrows_candidates() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("a.rs"), "fn main() {}\n").expect("write a.rs");
    fs::write(tmp.path().join("b.rs"), "fn other() {}\n").expect("write b.rs");
    fs::write(tmp.path().join("readme.md"), "Function-shaped fn-text inside markdown.\n")
        .expect("write readme");

    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-901",
        vec![hint(HintKind::PathPattern, "*.rs"), hint(HintKind::Regex, "\\bfn\\b")],
    );
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

    assert!(outcome.reserved_skipped.is_empty(), "no reserved kinds in this rule");
    assert_eq!(outcome.findings.len(), 2, "one finding per matched .rs file");
    let mut paths: Vec<&str> = outcome
        .findings
        .iter()
        .map(|f| f.location.as_ref().expect("location set").path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["a.rs", "b.rs"], "regex must only see path-pattern survivors");
    for finding in &outcome.findings {
        assert_eq!(finding.severity, Severity::Important);
        assert!(finding.fingerprint.starts_with("sha256:"));
    }
}

#[test]
fn path_pattern_empty_drops_findings() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("readme.md"), "fn-shaped content\n").expect("write readme");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-902",
        vec![hint(HintKind::PathPattern, "*.rs"), hint(HintKind::Regex, "fn")],
    );
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
    assert!(outcome.findings.is_empty(), "no .rs files survived path-pattern filter");
}
