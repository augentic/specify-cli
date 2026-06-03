//! Integration coverage for the framework tool declaration checks.

use std::fs;
use std::path::{Path, PathBuf};

use specify_standards::framework::check::tools::check_first_party_tools;
use specify_standards::framework::{Context, core_id_for, snippet};

fn scaffold_framework_root(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets dir");
    root.to_path_buf()
}

fn write_adapter(root: &Path, adapter: &str, contents: &str) {
    let dir = root.join("adapters/targets").join(adapter);
    fs::create_dir_all(&dir).expect("adapter dir");
    fs::write(dir.join("adapter.yaml"), contents).expect("adapter.yaml");
}

fn ctx_for(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root")
}

#[test]
fn invalid_tool_entry_shape_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = scaffold_framework_root(tmp.path());
    write_adapter(
        &root,
        "contracts",
        "tools:\n  - not-an-object\n  - name: contract\n    version: 0.3.0-rc.1\n",
    );
    write_adapter(&root, "vectis", "tools:\n  - name: vectis\n    version: 0.3.0\n");

    let findings = check_first_party_tools(&ctx_for(&root));
    assert_eq!(findings.len(), 2);
    assert!(
        findings.iter().all(|f| f.rule_id.as_deref() == core_id_for("tools.invalid-declaration"))
    );
    assert!(findings.iter().any(|f| snippet(f).contains("must be { name, version } objects")));
    assert!(findings.iter().any(|f| snippet(f).contains("'contract' package must be")));
}
