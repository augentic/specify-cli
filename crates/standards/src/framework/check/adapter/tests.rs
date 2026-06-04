use super::*;
use crate::framework::builder::{core_id_for, snippet};

#[test]
fn relative_path_strips_framework_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());
    let ctx = Context::from_framework_root(temp.path()).expect("framework root resolves");
    let path = ctx.sources_dir().join("intent").join(ADAPTER_FILENAME);
    assert_eq!(relative_path(&ctx, &path), "adapters/sources/intent/adapter.yaml");
}

#[test]
fn missing_manifest_on_empty_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());
    let adapter_dir = temp.path().join("adapters/sources/broken");
    fs::create_dir_all(&adapter_dir).expect("adapter dir");
    let ctx = Context::from_framework_root(temp.path()).expect("context");
    let findings = check_missing_manifests(&ctx, &ctx.sources_dir());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for(RULE_MISSING_MANIFEST));
    assert!(snippet(&findings[0]).contains("adapters/sources/broken"));
}

fn scaffold_framework(root: &Path) {
    fs::create_dir_all(root.join("plugins")).expect("plugins");
    fs::create_dir_all(root.join("adapters/sources")).expect("sources");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets");
}
