//! Smoke-test the specify-cli Rust-quality framework predicates.
//!
//! Run with `cargo test --test rust_quality`. During the naming burn-down
//! this may report findings without failing the smoke assertion.

use std::path::PathBuf;

use specify_standards::framework::check::run_rust_quality;
use specify_standards::framework::context::Context;

#[test]
fn rust_quality_predicates_run_on_workspace() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ctx = Context::from_specify_cli_root(&root).expect("specify-cli root");
    let findings = run_rust_quality(&ctx);
    assert!(!findings.is_empty(), "expected rust-quality findings during burn-down");
    assert!(
        findings.iter().all(|f| f.title.contains("rust.")),
        "unexpected finding titles: {:?}",
        findings.iter().map(|f| &f.title).collect::<Vec<_>>()
    );
}
