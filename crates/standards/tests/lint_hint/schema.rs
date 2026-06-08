//! Integration test for the `kind: schema` evaluator contract `schema` evaluator.
//!
//! Uses the bundled `rule` schema id token. A markdown file
//! whose frontmatter violates the schema (`severity: bogus` is not
//! in the closed enum) MUST yield at least one finding with a
//! `Structured` evidence payload carrying the failing JSON pointer.

use std::fs;

use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

#[test]
fn flags_invalid_frontmatter() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bad = "---\nid: UNI-999\ntitle: Bad\nseverity: bogus\ntrigger: trigger\n---\n## Rule\n";
    fs::write(tmp.path().join("rule.md"), bad).expect("write rule.md");

    let model = build(tmp.path(), ScanProfile::Project, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-904",
        vec![hint(HintKind::PathPattern, "rule.md"), hint(HintKind::Schema, "rule")],
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
fn resolves_registered_skill_schema() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Present frontmatter missing the required `description` key — the
    // `skill` registry entry must resolve and surface the violation.
    let bad = "---\nname: widget\n---\n# Body\n";
    fs::write(tmp.path().join("widget.md"), bad).expect("write widget.md");

    let model = build(tmp.path(), ScanProfile::Project, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-906",
        vec![hint(HintKind::PathPattern, "widget.md"), hint(HintKind::Schema, "skill")],
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

    assert!(
        !outcome.findings.is_empty(),
        "the registered `skill` schema id must resolve and flag the missing required field"
    );
}

#[test]
fn flags_scenario_schema_violation() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Opted-in scenario (leading `---`) missing required fields — the
    // `scenario` selector must validate the fact family whole-tree and
    // flag it, even though scenario files never enter `model.files`.
    let thin = "---\nid: thin\nstages: [refine, build]\n---\n\nBody.\n";
    let path = tmp.path().join("acceptance/scenarios/thin.md");
    fs::create_dir_all(path.parent().expect("parent")).expect("scenario dir");
    fs::write(&path, thin).expect("write scenario");

    let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-907", vec![hint(HintKind::Schema, "scenario")]);
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

    assert!(
        !outcome.findings.is_empty(),
        "a scenario missing required fields must flag the scenario schema selector"
    );
    assert!(
        outcome
            .findings
            .iter()
            .all(|f| f.location.as_ref().is_some_and(|l| l.path == "acceptance/scenarios/thin.md")),
        "findings must locate the offending scenario file"
    );
}

#[test]
fn valid_scenario_passes_schema_selector() {
    let tmp = tempfile::tempdir().expect("tmp");
    let ok = "---\nid: ok\nowner: spec\nkind: skill\nbackend: manual\nentrypoint: /spec:refine\nstages: [refine, build]\nisolation: fresh-project\n---\n\nBody.\n";
    let path = tmp.path().join("acceptance/scenarios/ok.md");
    fs::create_dir_all(path.parent().expect("parent")).expect("scenario dir");
    fs::write(&path, ok).expect("write scenario");

    let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-908", vec![hint(HintKind::Schema, "scenario")]);
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

    assert!(
        outcome.findings.is_empty(),
        "a schema-valid scenario flags nothing: {:?}",
        outcome.findings
    );
}

#[test]
fn flags_framework_toml_schema_violation() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bad = "cli = { version = \"not-a-version\" }\n";
    fs::write(tmp.path().join("Specify.toml"), bad).expect("write Specify.toml");

    let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-055",
        vec![
            hint(HintKind::PathPattern, "Specify.toml"),
            hint(HintKind::Schema, "framework"),
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

    assert!(
        !outcome.findings.is_empty(),
        "invalid Specify.toml version must flag the framework schema"
    );
}

#[test]
fn valid_framework_toml_passes_schema() {
    let tmp = tempfile::tempdir().expect("tmp");
    let ok = "cli = { version = \"0.1.0\" }\n";
    fs::write(tmp.path().join("Specify.toml"), ok).expect("write Specify.toml");

    let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-055",
        vec![
            hint(HintKind::PathPattern, "Specify.toml"),
            hint(HintKind::Schema, "framework"),
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

    assert!(
        outcome.findings.is_empty(),
        "schema-valid Specify.toml flags nothing: {:?}",
        outcome.findings
    );
}

#[test]
fn schema_hint_rejects_http_reference() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("x.json"), "{}").expect("write");
    let model = build(tmp.path(), ScanProfile::Project, &[], &[]).expect("build");
    let rule =
        make_rule("UNI-905", vec![hint(HintKind::Schema, "https://example.com/schema.json")]);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let err = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect_err("http schema refs are refused");
    assert!(format!("{err}").contains("http"), "error must mention the http rejection: {err}");
}
