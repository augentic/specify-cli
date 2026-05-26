use std::fs;
use std::path::{Path, PathBuf};

use specify_authoring::Context;
use specify_authoring::check::codex_schema_drift::{self, RULE_SCHEMA_DRIFT};

const AUTHORING_REL: &str = "crates/authoring/schemas/codex-rule.schema.json";
const VENDORED_REL: &str = "schemas/codex/codex-rule.schema.json";

const SCHEMA_A: &str =
    "{\"$schema\":\"https://json-schema.org/draft/2020-12/schema\",\"title\":\"A\"}\n";
const SCHEMA_B: &str =
    "{\"$schema\":\"https://json-schema.org/draft/2020-12/schema\",\"title\":\"B\"}\n";

fn scaffold_framework_root(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters")).expect("adapters dir");
    root.to_path_buf()
}

fn write(path: PathBuf, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, body).expect("write schema");
}

fn ctx_for(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root")
}

#[test]
fn neither_schema_present_no_findings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());

    let findings = codex_schema_drift::run(&ctx_for(&root));
    assert!(findings.is_empty(), "expected no findings, got {findings:?}");
}

#[test]
fn authoring_only_no_findings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write(root.join(AUTHORING_REL), SCHEMA_A);

    let findings = codex_schema_drift::run(&ctx_for(&root));
    assert!(findings.is_empty(), "expected no findings, got {findings:?}");
}

#[test]
fn vendored_without_authoring_emits_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write(root.join(VENDORED_REL), SCHEMA_A);

    let findings = codex_schema_drift::run(&ctx_for(&root));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id, RULE_SCHEMA_DRIFT);
    assert!(
        findings[0].message.contains("authoring source-of-truth"),
        "unexpected message: {}",
        findings[0].message
    );
    assert!(findings[0].message.contains("scripts/sync-codex-schema.sh"));
}

#[test]
fn drift_emits_finding_then_clears_after_resync() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write(root.join(AUTHORING_REL), SCHEMA_A);
    write(root.join(VENDORED_REL), SCHEMA_B);

    let findings = codex_schema_drift::run(&ctx_for(&root));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id, RULE_SCHEMA_DRIFT);
    assert!(
        findings[0].message.contains("diverged"),
        "unexpected message: {}",
        findings[0].message
    );
    assert!(findings[0].message.contains("scripts/sync-codex-schema.sh"));

    fs::write(root.join(VENDORED_REL), SCHEMA_A).expect("resync vendored");
    let findings = codex_schema_drift::run(&ctx_for(&root));
    assert!(findings.is_empty(), "expected no findings after resync, got {findings:?}");
}
