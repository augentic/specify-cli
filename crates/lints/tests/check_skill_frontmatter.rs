use std::fs;
use std::path::Path;

use specify_authoring::Context;
use specify_authoring::check::{
    ArgumentHintGrammar, DescriptionGrammar, FrontmatterSchema, NameDirMismatch, RULE_UNKNOWN_TOOL,
    SKILL_RULE_SCHEMA_VIOLATION, UnknownTool,
};
use specify_authoring::finding::Check;

fn fixture_context(root: &Path) -> Context {
    Context::from_framework_root(root).expect("fixture framework root")
}

fn write_framework_scaffold(root: &Path) {
    fs::create_dir_all(root.join("adapters")).expect("adapters dir");
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
}

fn write_skill(root: &Path, plugin: &str, skill: &str, frontmatter: &str) {
    let dir = root.join("plugins").join(plugin).join("skills").join(skill);
    fs::create_dir_all(&dir).expect("skill dir");
    fs::write(dir.join("SKILL.md"), format!("---\n{frontmatter}\n---\n\n# Test\n"))
        .expect("skill md");
}

#[test]
fn schema_reports_missing_use_when() {
    let temp = tempfile::tempdir().expect("temp dir");
    write_framework_scaffold(temp.path());
    write_skill(
        temp.path(),
        "demo",
        "bad-description",
        "name: demo-bad-description\ndescription: Too short.",
    );

    let ctx = fixture_context(temp.path());
    let findings = FrontmatterSchema.run(&ctx);
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id == SKILL_RULE_SCHEMA_VIOLATION
                && finding.message.contains("bad-description")
                && finding.message.contains("/description")
        }),
        "expected schema violation for description, got {findings:?}"
    );
}

#[test]
fn unknown_tool_reports_disallowed() {
    let temp = tempfile::tempdir().expect("temp dir");
    write_framework_scaffold(temp.path());
    write_skill(
        temp.path(),
        "demo",
        "bad-tools",
        "name: demo-bad-tools\ndescription: Build demo fixtures for tool validation. Use when checking allowed-tools whitelisting.\nallowed-tools: NotARealTool",
    );

    let ctx = fixture_context(temp.path());
    let findings = UnknownTool.run(&ctx);
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id == RULE_UNKNOWN_TOOL && finding.message.contains("NotARealTool")
        }),
        "expected unknown-tool finding, got {findings:?}"
    );
}

#[test]
fn spec_prefix_override_accepts_specify_prefix() {
    let temp = tempfile::tempdir().expect("temp dir");
    write_framework_scaffold(temp.path());
    write_skill(
        temp.path(),
        "spec",
        "init",
        "name: specify-init\ndescription: Initialize Specify in a project. Use when first wiring up a project before any other slash command.\nargument-hint: <adapter>",
    );

    let ctx = fixture_context(temp.path());
    assert!(FrontmatterSchema.run(&ctx).is_empty());
    assert!(NameDirMismatch.run(&ctx).is_empty());
    assert!(DescriptionGrammar.run(&ctx).is_empty());
    assert!(ArgumentHintGrammar.run(&ctx).is_empty());
}
