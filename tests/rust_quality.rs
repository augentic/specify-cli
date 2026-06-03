//! Enforce the specify-cli Rust-quality framework predicates.
//!
//! Run with `cargo test --test rust_quality`. The test-naming cap is a hard
//! gate: any `rust.test-fn-name-too-long` finding fails CI. The archaeology
//! predicate (`RustSourceQuality`) remains burn-down tracked in
//! `docs/quality-debt.md` and is not asserted here.

use std::path::PathBuf;

use specify_standards::framework::check::run_rust_quality;
use specify_standards::framework::context::Context;

const TEST_NAMING_RULE: &str = "rust.test-fn-name-too-long";

#[test]
fn no_long_test_fn_names() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ctx = Context::from_specify_cli_root(&root).expect("specify-cli root");
    let offenders: Vec<String> = run_rust_quality(&ctx)
        .into_iter()
        .filter(|f| f.title.contains(TEST_NAMING_RULE))
        .map(|f| f.title)
        .collect();
    assert!(
        offenders.is_empty(),
        "test fn names must be <= 40 chars (see docs/standards/testing.md); offenders: {offenders:#?}"
    );
}
