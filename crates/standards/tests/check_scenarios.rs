//! Integration coverage for the framework scenario frontmatter checks.

use std::fs;
use std::path::{Path, PathBuf};

use specify_standards::framework::check::{
    RULE_RECORDED_TRACE_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS, SCENARIO_RULE_ARTIFACT_PATH_UNSAFE,
    SCENARIO_RULE_SCHEMA_VIOLATION, check_recorded_trace_freshness, validate_scenario_frontmatter,
};
use specify_standards::framework::{Context, core_id_for, snippet};
use tempfile::TempDir;

fn scaffold_framework_root(base: &Path) -> PathBuf {
    let root = base.join("framework");
    fs::create_dir_all(root.join("plugins/spec")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets dir");
    root
}

fn write_scenario(root: &Path, rel: &str, content: &str) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("scenario parent dir");
    }
    fs::write(&path, content).expect("write scenario");
    path
}

fn context_for(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root resolves")
}

const VALID_FRONTMATTER: &str = r"---
id: demo-scenario
owner: demo
kind: suite
backend: manual
entrypoint: /spec:plan
stages: [plan]
isolation: fresh-project
---
";

#[test]
fn schema_violation_missing_field() {
    let tmp = TempDir::new().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write_scenario(
        &root,
        "acceptance/suites/demo/scenario.md",
        &format!("{VALID_FRONTMATTER}\n# Demo\n\nScenario ID: `demo-scenario`\n")
            .replace("owner: demo\n", ""),
    );

    let findings = validate_scenario_frontmatter(&context_for(&root));
    let schema: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_deref() == core_id_for(SCENARIO_RULE_SCHEMA_VIOLATION))
        .collect();
    assert!(!schema.is_empty(), "expected schema violation, got: {findings:?}");
    assert!(
        schema.iter().any(|f| snippet(f).contains("Scenario frontmatter:")),
        "expected Deno-shaped message prefix, got: {findings:?}"
    );
}

#[test]
fn stages_not_contiguous_emits_finding() {
    let tmp = TempDir::new().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write_scenario(
        &root,
        "acceptance/suites/demo/scenario.md",
        &format!("{VALID_FRONTMATTER}\n# Demo\n")
            .replace("stages: [plan]", "stages: [plan, build]"),
    );

    let findings = validate_scenario_frontmatter(&context_for(&root));
    let stages: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_deref() == core_id_for(RULE_STAGES_NOT_CONTIGUOUS))
        .collect();
    assert_eq!(stages.len(), 1, "expected one stages finding, got: {findings:?}");
    assert!(snippet(stages[0]).contains("contiguous slice"));
}

#[test]
fn path_unsafe_rejects_parent_escape() {
    let tmp = TempDir::new().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write_scenario(
        &root,
        "acceptance/suites/demo/scenario.md",
        &format!("{VALID_FRONTMATTER}\n# Demo\n").replace(
            "isolation: fresh-project",
            "isolation: fresh-project\nexpected-artifacts:\n  - ../escape.yaml",
        ),
    );

    let findings = validate_scenario_frontmatter(&context_for(&root));
    let art: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_deref() == core_id_for(SCENARIO_RULE_ARTIFACT_PATH_UNSAFE))
        .collect();
    assert_eq!(art.len(), 1, "expected one artifact finding, got: {findings:?}");
    assert!(snippet(art[0]).contains("'..' segment not allowed"));
}

#[test]
fn recorded_trace_invalid_header() {
    let tmp = TempDir::new().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    let trace = root.join("acceptance/recorded/demo.jsonl");
    fs::create_dir_all(trace.parent().unwrap()).expect("recorded dir");
    fs::write(&trace, r#"{"kind":"wrong"}"#).expect("bad trace");

    let findings = check_recorded_trace_freshness(&context_for(&root));
    assert!(
        findings.iter().any(|f| {
            f.rule_id.as_deref() == core_id_for(RULE_RECORDED_TRACE_VIOLATION)
                && snippet(f).contains("kind must be 'recorded-trace-header'")
        }),
        "expected kind violation, got: {findings:?}"
    );
}
