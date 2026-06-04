//! Integration coverage for the framework prose vocabulary/cap checks.

use std::fs;
use std::path::{Path, PathBuf};

use specify_standards::framework::check::{Check, NumericCaps};
use specify_standards::framework::{Context, core_id_for, snippet};

fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/prose").join(name)
}

fn scaffold_framework_root(root: &Path) {
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters")).expect("adapters dir");
}

fn context_for_fixture(name: &str) -> Context {
    let root = fixture_root(name);
    scaffold_framework_root(&root);
    Context::from_framework_root(root).expect("framework root resolves")
}

#[test]
fn skill_numeric_caps_detects_drift() {
    let ctx = context_for_fixture("cap-drift");
    let findings = NumericCaps.run(&ctx);
    assert_eq!(findings.len(), 2);
    assert!(
        findings.iter().all(|f| f.rule_id.as_deref() == core_id_for("prose.numeric-cap-exceeded"))
    );
    assert!(findings.iter().any(|f| snippet(f).contains("description cap drift")));
    assert!(findings.iter().any(|f| snippet(f).contains("body cap drift")));
}
