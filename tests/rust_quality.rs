//! Enforce the specify-cli Rust-quality framework predicates.
//!
//! Run with `cargo test --test rust_quality`. Hard gates: any
//! `rust.test-fn-name-too-long`, `rust.workflow-clock-read`, or
//! `rust.allow-without-reason` finding fails CI. The archaeology
//! predicate (`rust.archaeology-in-doc-comment`) stays burn-down tracked
//! in `docs/quality-debt.md` — its `RFC-`/`Phase ` markers over-fire on
//! the canonical contract vocabulary the codebase and AGENTS.md use, so
//! it is not gated.

use std::path::PathBuf;

use specify_standards::framework::check::run_rust_quality;
use specify_standards::framework::context::Context;

const TEST_NAMING_RULE: &str = "rust.test-fn-name-too-long";
const WORKFLOW_CLOCK_RULE: &str = "rust.workflow-clock-read";
const ALLOW_NO_REASON_RULE: &str = "rust.allow-without-reason";

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

#[test]
fn no_clock_reads_in_workflow_library() {
    // Time injection (architecture §Time injection): `specify-workflow`
    // must accept an injected `now`; the clock is read once in a
    // `src/runtime/commands/**` handler and threaded down.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ctx = Context::from_specify_cli_root(&root).expect("specify-cli root");
    let offenders: Vec<String> = run_rust_quality(&ctx)
        .into_iter()
        .filter(|f| f.title.contains(WORKFLOW_CLOCK_RULE))
        .map(|f| f.title)
        .collect();
    assert!(
        offenders.is_empty(),
        "specify-workflow library code must not call `Timestamp::now()` (see docs/standards/architecture.md §Time injection); offenders: {offenders:#?}"
    );
}

#[test]
fn no_bare_allow_attributes() {
    // `#[allow]` without a `reason` is forbidden (style.md §Lint
    // suppression posture): use `#[expect(.., reason = "…")]` at the
    // smallest scope, or a contract-locked module `#![allow]`.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ctx = Context::from_specify_cli_root(&root).expect("specify-cli root");
    let offenders: Vec<String> = run_rust_quality(&ctx)
        .into_iter()
        .filter(|f| f.title.contains(ALLOW_NO_REASON_RULE))
        .map(|f| f.title)
        .collect();
    assert!(
        offenders.is_empty(),
        "`#[allow]` must carry a reason or be an `#[expect]` (see docs/standards/style.md); offenders: {offenders:#?}"
    );
}
