//! Smoke test running the framework `Check` pass against a live root.

use specify_standards::framework::Context;

fn framework_root() -> Option<std::path::PathBuf> {
    std::env::var("SPECDEV_FRAMEWORK_ROOT").ok().map(std::path::PathBuf::from)
}

#[test]
fn framework_root_from_explicit_path() {
    let Some(root) = framework_root() else {
        return;
    };
    let ctx = Context::from_framework_root(&root).expect("framework root resolves");
    assert!(ctx.plugins_dir().join("spec").is_dir());
    assert!(ctx.specify_cli_schemas_dir().ends_with("schemas"));
}

#[test]
fn check_runs_with_no_findings() {
    let Some(root) = framework_root() else {
        return;
    };
    let ctx = Context::from_framework_root(root).expect("framework root resolves");
    let findings = specify_standards::framework::check::run(&ctx);
    assert!(findings.is_empty());
}
