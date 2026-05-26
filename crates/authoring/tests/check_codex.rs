use std::fs;
use std::path::Path;

use specify_authoring::Context;
use specify_authoring::check::codex::{
    RULE_DUPLICATE_RULE_ID, RULE_NAMESPACE_OWNERSHIP_VIOLATION, RULE_SCHEMA_VIOLATION,
    run_codex_check,
};

const CODEX_RULE_PREFIX: &str = "codex.";

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
fn src_rule_under_source_adapter_passes_full_codex_check() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_codex_rule(
        temp.path(),
        "adapters/sources/documentation/codex/some-rule.md",
        &valid_rule("SRC-999"),
    );

    let findings = run_codex_check(&ctx_for(temp.path()));
    let codex_findings: Vec<_> =
        findings.iter().filter(|finding| finding.rule_id.starts_with(CODEX_RULE_PREFIX)).collect();
    assert!(
        codex_findings.is_empty(),
        "expected no codex.* findings for a valid SRC-* rule under a source adapter, got: {codex_findings:?}",
    );
}

#[test]
fn frame_rule_under_target_adapter_rejected_by_integration_check() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_codex_rule(
        temp.path(),
        "adapters/targets/omnia/codex/frame-misplaced.md",
        &valid_rule("FRAME-001"),
    );

    let findings = run_codex_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION
                && finding.message.contains("FRAME-*")
                && finding.message.contains("RFC-32")
                && finding.message.contains("FRAME-001")
        }),
        "expected FRAME-* ownership violation citing the RFC-32 reservation, got: {findings:?}",
    );
}

#[test]
fn reserved_hint_kind_shape_validates_without_schema_finding() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_framework_scaffold(temp.path());
    write_codex_rule(
        temp.path(),
        "adapters/shared/codex/universal/reserved-hint.md",
        r#"---
id: UNI-999
title: Reserved Hint Shape Test
severity: important
trigger: When validating that RFC-32 reserved hint kinds shape-check only.
deterministic_hints:
  - kind: namespace-owner
    value: adapters/shared/codex/universal
  - kind: set-coverage
    value: SRC,UNI,OMNIA,VECTIS,IFACE,RUST,SEC,ORG,FRAME
---

## Rule

Body for a rule that exercises RFC-32 reserved deterministic hint kinds. The
authoring schema accepts the kinds; `check::codex` performs shape validation
only and does not execute the hints.
"#,
    );

    let findings = run_codex_check(&ctx_for(temp.path()));
    let schema_findings: Vec<_> =
        findings.iter().filter(|finding| finding.rule_id == RULE_SCHEMA_VIOLATION).collect();
    assert!(
        schema_findings.is_empty(),
        "reserved hint kinds should pass shape validation without codex.schema-violation findings, got: {schema_findings:?}",
    );
    let ownership_findings: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION)
        .collect();
    assert!(
        ownership_findings.is_empty(),
        "UNI-* under adapters/shared/codex/universal must not trip namespace ownership, got: {ownership_findings:?}",
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
