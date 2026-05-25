use std::fs;
use std::path::Path;

use specify_authoring::Context;
use specify_authoring::check::codex::{
    RULE_DUPLICATE_RULE_ID, RULE_NAMESPACE_OWNERSHIP_VIOLATION, RULE_SCHEMA_VIOLATION,
    run_codex_check,
};

fn write_framework_scaffold(root: &Path) {
    fs::create_dir_all(root.join("adapters/sources")).expect("sources dir");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets dir");
    fs::create_dir_all(root.join("adapters/shared")).expect("shared dir");
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
}

fn write_codex_rule(root: &Path, rel_path: &str, body: &str) {
    let path = root.join(rel_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("codex rule parent dir");
    }
    fs::write(path, body).expect("codex rule");
}

fn valid_rule(id: &str) -> String {
    format!(
        r#"---
id: {id}
title: Test Rule
severity: important
trigger: When testing codex validation in specdev check.
---

## Rule

Keep the rule body present for validation.
"#
    )
}

fn ctx_for(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root")
}

#[test]
fn schema_violation_on_invalid_frontmatter() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_codex_rule(
        temp.path(),
        "adapters/shared/codex/universal/bad-frontmatter.md",
        r#"---
id: UNI-001
title: Missing required fields
---

## Rule

Body without trigger and severity.
"#,
    );

    let findings = run_codex_check(&ctx_for(temp.path()));
    let schema_findings: Vec<_> =
        findings.iter().filter(|finding| finding.rule_id == RULE_SCHEMA_VIOLATION).collect();
    assert!(!schema_findings.is_empty(), "expected schema violation findings, got: {findings:?}");
    assert!(
        schema_findings.iter().any(|finding| finding.message.contains("Codex rule frontmatter:")),
        "expected frontmatter validation message, got: {findings:?}"
    );
}

#[test]
fn namespace_ownership_violation_on_wrong_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_codex_rule(
        temp.path(),
        "adapters/targets/omnia/codex/wrong-namespace.md",
        &valid_rule("VECTIS-001"),
    );

    let findings = run_codex_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION
                && finding.message.contains("codex owner 'omnia' may only use")
                && finding.message.contains("VECTIS-001")
        }),
        "expected namespace ownership finding, got: {findings:?}"
    );
}

#[test]
fn duplicate_rule_id_across_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_codex_rule(
        temp.path(),
        "adapters/shared/codex/universal/first-duplicate.md",
        &valid_rule("UNI-003"),
    );
    write_codex_rule(
        temp.path(),
        "adapters/shared/codex/universal/second-duplicate.md",
        &valid_rule("UNI-003"),
    );

    let findings = run_codex_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id == RULE_DUPLICATE_RULE_ID
                && finding.message.contains("UNI-003")
                && finding.message.contains("first-duplicate.md")
                && finding.message.contains("second-duplicate.md")
        }),
        "expected duplicate id finding, got: {findings:?}"
    );
}
