//! Integration coverage for the framework rule schema/namespace checks.

use std::fs;
use std::path::Path;

use specify_lints::framework::check::rules::{
    RULE_DUPLICATE_RULE_ID, RULE_NAMESPACE_OWNERSHIP_VIOLATION, RULE_SCHEMA_VIOLATION,
    run_rules_check,
};
use specify_lints::framework::{Context, core_id_for, snippet};

fn write_framework_scaffold(root: &Path) {
    fs::create_dir_all(root.join("adapters/sources")).expect("sources dir");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets dir");
    fs::create_dir_all(root.join("adapters/shared")).expect("shared dir");
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
}

fn write_rule_file(root: &Path, rel_path: &str, body: &str) {
    let path = root.join(rel_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("rule parent dir");
    }
    fs::write(path, body).expect("rule");
}

fn valid_rule(id: &str) -> String {
    format!(
        r#"---
id: {id}
title: Test Rule
severity: important
trigger: When testing codex validation in specdev lint.
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
    write_rule_file(
        temp.path(),
        "adapters/shared/rules/universal/bad-frontmatter.md",
        r#"---
id: UNI-001
title: Missing required fields
---

## Rule

Body without trigger and severity.
"#,
    );

    let findings = run_rules_check(&ctx_for(temp.path()));
    let schema_findings: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule_id.as_deref() == core_id_for(RULE_SCHEMA_VIOLATION))
        .collect();
    assert!(!schema_findings.is_empty(), "expected schema violation findings, got: {findings:?}");
    assert!(
        schema_findings.iter().any(|finding| snippet(finding).contains("Rule frontmatter:")),
        "expected frontmatter validation message, got: {findings:?}"
    );
}

#[test]
fn namespace_violation_wrong_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_rule_file(
        temp.path(),
        "adapters/targets/omnia/rules/wrong-namespace.md",
        &valid_rule("VECTIS-001"),
    );

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("rules owner 'omnia' may only use")
                && snippet(finding).contains("VECTIS-001")
        }),
        "expected namespace ownership finding, got: {findings:?}"
    );
}

#[test]
fn src_rule_on_source_passes_codex_check() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_rule_file(
        temp.path(),
        "adapters/sources/documentation/rules/some-rule.md",
        &valid_rule("SRC-999"),
    );

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.is_empty(),
        "expected no rules-check findings for a valid SRC-* rule under a source adapter, got: {findings:?}",
    );
}

#[test]
fn frame_rule_on_target_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_rule_file(
        temp.path(),
        "adapters/targets/omnia/rules/frame-misplaced.md",
        &valid_rule("FRAME-001"),
    );

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("FRAME-*")
                && snippet(finding).contains("framework-repo declarative rules")
                && snippet(finding).contains("FRAME-001")
        }),
        "expected FRAME-* ownership violation citing the framework rule-namespace reservation, got: {findings:?}",
    );
}

#[test]
fn reserved_hint_kind_no_schema_finding() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_rule_file(
        temp.path(),
        "adapters/shared/rules/universal/reserved-hint.md",
        r#"---
id: UNI-999
title: Reserved Hint Shape Test
severity: important
trigger: When validating that Reserved hint kind hint kinds shape-check only.
deterministic_hints:
  - kind: namespace-owner
    value: adapters/shared/rules/universal
  - kind: set-coverage
    value: SRC,UNI,OMNIA,VECTIS,IFACE,RUST,SEC,ORG,FRAME
---

## Rule

Body for a rule that exercises Reserved hint kind deterministic hint kinds. The
authoring schema accepts the kinds; `check::rules` performs shape validation
only and does not execute the hints.
"#,
    );

    let findings = run_rules_check(&ctx_for(temp.path()));
    let schema_findings: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule_id.as_deref() == core_id_for(RULE_SCHEMA_VIOLATION))
        .collect();
    assert!(
        schema_findings.is_empty(),
        "reserved hint kinds should pass shape validation without rules.schema-violation findings, got: {schema_findings:?}",
    );
    let ownership_findings: Vec<_> = findings
        .iter()
        .filter(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
        })
        .collect();
    assert!(
        ownership_findings.is_empty(),
        "UNI-* under adapters/shared/rules/universal must not trip namespace ownership, got: {ownership_findings:?}",
    );
}

#[test]
fn duplicate_rule_id_across_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_rule_file(
        temp.path(),
        "adapters/shared/rules/universal/first-duplicate.md",
        &valid_rule("UNI-003"),
    );
    write_rule_file(
        temp.path(),
        "adapters/shared/rules/universal/second-duplicate.md",
        &valid_rule("UNI-003"),
    );

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_DUPLICATE_RULE_ID)
                && snippet(finding).contains("UNI-003")
                && snippet(finding).contains("first-duplicate.md")
                && snippet(finding).contains("second-duplicate.md")
        }),
        "expected duplicate id finding, got: {findings:?}"
    );
}
