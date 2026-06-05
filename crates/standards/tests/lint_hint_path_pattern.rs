//! Integration test for the the executable hint-kind contract
//! `path-pattern` evaluator.
//!
//! `path-pattern` is a candidate-set filter, not a finder. The lint contract
//! contract is verified indirectly: a rule with `path-pattern: *.rs`
//! plus `regex: fn` MUST emit findings only for the `*.rs` files
//! reachable from the workspace model.

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

    let model = build(tmp.path(), ScanProfile::Product, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-901",
        vec![hint(HintKind::PathPattern, "*.rs"), hint(HintKind::Regex, "\\bfn\\b")],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;

    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

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
    let model = build(tmp.path(), ScanProfile::Product, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-902",
        vec![hint(HintKind::PathPattern, "*.rs"), hint(HintKind::Regex, "fn")],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");
    assert!(outcome.findings.is_empty(), "no .rs files survived path-pattern filter");
}

#[test]
fn path_pattern_exclusion_carves_out_paths() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("docs/explanation")).expect("mkdir");
    fs::write(tmp.path().join("docs/bad.md"), "fn main() {}\n").expect("write bad");
    fs::write(tmp.path().join("docs/explanation/decision-log.md"), "fn other() {}\n")
        .expect("write allowlisted");

    let model = build(tmp.path(), ScanProfile::Product, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-903",
        vec![
            hint(HintKind::PathPattern, "docs/**/*.md"),
            hint(HintKind::PathPattern, "!docs/explanation/decision-log.md"),
            hint(HintKind::Regex, "\\bfn\\b"),
        ],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

    assert_eq!(outcome.findings.len(), 1);
    assert_eq!(outcome.findings[0].location.as_ref().expect("loc").path, "docs/bad.md");
}
