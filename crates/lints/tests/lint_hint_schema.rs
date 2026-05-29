//! Integration test for the `kind: schema` evaluator contract `schema` evaluator.
//!
//! Uses the bundled `rule` schema id token. A markdown file
//! whose frontmatter violates the schema (`severity: bogus` is not
//! in the closed enum) MUST yield at least one finding with a
//! `Structured` evidence payload carrying the failing JSON pointer.

mod eval_support;

use std::fs;

use eval_support::{NoToolRunner, hint, make_rule};
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{FindingEvidence, HintKind};

#[test]
fn flags_invalid_frontmatter() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bad = "---\nid: UNI-999\ntitle: Bad\nseverity: bogus\ntrigger: trigger\n---\n## Rule\n";
    fs::write(tmp.path().join("rule.md"), bad).expect("write rule.md");

    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-904",
        vec![hint(HintKind::PathPattern, "rule.md"), hint(HintKind::Schema, "rule")],
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

    assert!(
        !outcome.findings.is_empty(),
        "schema validation must emit at least one finding for bogus severity"
    );
    let cited_severity = outcome.findings.iter().any(|f| match &f.evidence {
        FindingEvidence::Structured { summary, data, .. } => {
            summary.contains("severity") || data.to_string().contains("severity")
        }
        _ => false,
    });
    assert!(cited_severity, "at least one finding must cite the failing `severity` keyword");
}

#[test]
fn schema_hint_rejects_http_reference() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("x.json"), "{}").expect("write");
    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build");
    let rule =
        make_rule("UNI-905", vec![hint(HintKind::Schema, "https://example.com/schema.json")]);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let err = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect_err("http schema refs are refused");
    assert!(format!("{err}").contains("http"), "error must mention the http rejection: {err}");
}
