//! Smoke test running the framework `Check` pass against a live root.

use specify_standards::framework::Context;
use specify_standards::framework::check::run_rust_quality;

fn framework_root() -> Option<std::path::PathBuf> {
    std::env::var("SPECIFY_FRAMEWORK_ROOT").ok().map(std::path::PathBuf::from)
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
fn rust_quality_runs_with_no_findings() {
    let Some(root) = framework_root() else {
        return;
    };
    let ctx = Context::from_framework_root(root).expect("framework root resolves");
    // The plugin framework root carries no Rust sources, so the
    // Rust-quality predicates are no-ops there. (The CORE-009 namespace
    // producer this smoke test used to drive moved to the `rules` WASI
    // tool in Phase 7.)
    let findings = run_rust_quality(&ctx);
    assert!(findings.is_empty());
}
