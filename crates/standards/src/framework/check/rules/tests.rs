use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::framework::builder::{core_id_for, snippet};

#[test]
fn namespace_for_rule_id_extracts_prefix() {
    assert_eq!(namespace_for_rule_id("UNI-014"), Some("UNI"));
    assert_eq!(namespace_for_rule_id("OMNIA-001"), Some("OMNIA"));
    assert_eq!(namespace_for_rule_id("bad"), None);
}

#[test]
fn namespace_list_formats_wildcards() {
    let namespaces = HashSet::from(["OMNIA", "RUST", "SEC"]);
    assert_eq!(namespace_list(&namespaces), "OMNIA-*, RUST-*, SEC-*");
}

fn scaffold_framework(root: &Path) {
    fs::create_dir_all(root.join("adapters/sources")).expect("sources dir");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets dir");
    fs::create_dir_all(root.join("adapters/shared")).expect("shared dir");
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
}

fn write_rule(root: &Path, rel: &str, id: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("rule parent dir")).expect("create parent");
    let body = format!(
        "---\nid: {id}\ntitle: Test Rule\nseverity: important\ntrigger: When testing codex validation in specdev lint.\n---\n\n## Rule\n\nBody.\n"
    );
    fs::write(path, body).expect("write rule");
}

fn ctx_for(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root")
}

#[test]
fn owners_merge_builtins_and_discovered() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    fs::create_dir_all(temp.path().join("adapters/sources/documentation/rules"))
        .expect("documentation rules");
    fs::create_dir_all(temp.path().join("adapters/sources/captures/rules"))
        .expect("captures rules");
    fs::create_dir_all(temp.path().join("adapters/sources/intent")).expect("intent no rules");

    let ctx = ctx_for(temp.path());
    let owners = namespace_owners(&ctx);

    assert_eq!(owners.get("documentation"), Some(&HashSet::from(["SRC"])));
    assert_eq!(owners.get("captures"), Some(&HashSet::from(["SRC"])));
    assert!(
        !owners.contains_key("intent"),
        "intent has no rules/ subtree so it must not be registered",
    );
    assert_eq!(owners.get(SHARED_RULES_OWNER), Some(&HashSet::from(["UNI"])));
    assert_eq!(owners.get("omnia"), Some(&HashSet::from(["OMNIA", "RUST", "SEC"])));
    assert_eq!(owners.get("vectis"), Some(&HashSet::from(["VECTIS"])));
    assert_eq!(owners.get("contracts"), Some(&HashSet::from(["IFACE"])));
}

#[test]
fn src_rule_on_source_passes() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/sources/documentation/rules/source-overlay.md", "SRC-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    let ownership: Vec<_> = findings
        .iter()
        .filter(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
        })
        .collect();
    assert!(
        ownership.is_empty(),
        "SRC-* under source-adapter rules should pass, got: {ownership:?}",
    );
}

#[test]
fn non_src_rule_under_source_adapter_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/sources/documentation/rules/wrong-namespace.md", "OMNIA-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("rules owner 'documentation' may only use")
                && snippet(finding).contains("SRC-*")
                && snippet(finding).contains("OMNIA-001")
        }),
        "expected SRC-only enforcement under source adapter, got: {findings:?}",
    );
}

#[test]
fn frame_rule_on_target_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/targets/omnia/rules/frame-misplaced.md", "FRAME-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("FRAME-*")
                && snippet(finding).contains("framework-repo declarative rules")
                && snippet(finding).contains("FRAME-001")
                && snippet(finding).contains("omnia")
        }),
        "expected FRAME placement violation with framework rule-namespace reservation message, got: {findings:?}",
    );
}

#[test]
fn frame_rule_on_source_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/sources/documentation/rules/frame-misplaced.md", "FRAME-007");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("FRAME-*")
                && snippet(finding).contains("framework-repo declarative rules")
                && snippet(finding).contains("FRAME-007")
                && snippet(finding).contains("documentation")
        }),
        "expected FRAME placement violation under source adapter, got: {findings:?}",
    );
}

#[test]
fn core_rule_under_core_pack_passes() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/shared/rules/core/CORE-fixture.md", "CORE-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    let ownership: Vec<_> = findings
        .iter()
        .filter(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
        })
        .collect();
    assert!(
        ownership.is_empty(),
        "CORE-* under adapters/shared/rules/core/ should pass, got: {ownership:?}",
    );
}

#[test]
fn core_rule_under_target_adapter_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/targets/omnia/rules/core-misplaced.md", "CORE-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("rules owner 'omnia' may only use")
                && snippet(finding).contains("OMNIA-*")
                && snippet(finding).contains("CORE-001")
        }),
        "expected CORE-* under target adapter to be rejected, got: {findings:?}",
    );
}

#[test]
fn core_rule_under_source_adapter_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/sources/documentation/rules/core-misplaced.md", "CORE-007");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("rules owner 'documentation' may only use")
                && snippet(finding).contains("SRC-*")
                && snippet(finding).contains("CORE-007")
        }),
        "expected CORE-* under source adapter to be rejected, got: {findings:?}",
    );
}

#[test]
fn non_core_rule_under_core_pack_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/shared/rules/core/foreign.md", "UNI-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("rules owner 'core' may only use")
                && snippet(finding).contains("CORE-*")
                && snippet(finding).contains("UNI-001")
        }),
        "expected non-CORE-* under core pack to be rejected, got: {findings:?}",
    );
}

#[test]
fn vectis_overlay_rust_id_rejected() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_rule(temp.path(), "adapters/targets/vectis/rules/rust-misplaced.md", "RUST-001");

    let findings = run_rules_check(&ctx_for(temp.path()));
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                && snippet(finding).contains("rules owner 'vectis' may only use")
                && snippet(finding).contains("VECTIS-*")
                && snippet(finding).contains("RUST-001")
        }),
        "expected vectis to keep rejecting non-VECTIS ids, got: {findings:?}",
    );
}
