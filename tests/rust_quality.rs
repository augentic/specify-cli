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

/// The gated rules and the standards-doc pointer rendered when one fires.
const GATED_RULES: [(&str, &str); 3] = [
    (
        "rust.test-fn-name-too-long",
        "test fn names must be <= 40 chars (see docs/standards/testing.md)",
    ),
    (
        // Time injection (architecture §Time injection): `specify-workflow`
        // must accept an injected `now`; the clock is read once in a
        // `src/runtime/commands/**` handler and threaded down.
        "rust.workflow-clock-read",
        "specify-workflow library code must not call `Timestamp::now()` (see docs/standards/architecture.md §Time injection)",
    ),
    (
        // `#[allow]` without a `reason` is forbidden (style.md §Lint
        // suppression posture): use `#[expect(.., reason = "…")]` at the
        // smallest scope, or a contract-locked module `#![allow]`.
        "rust.allow-without-reason",
        "`#[allow]` must carry a reason or be an `#[expect]` (see docs/standards/style.md)",
    ),
];

#[test]
fn no_gated_rust_quality_findings() {
    // One repo scan; findings grouped per rule id so a failure stays
    // attributable to the standard it breaches.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ctx = Context::from_specify_cli_root(&root).expect("specify-cli root");
    let findings = run_rust_quality(&ctx);

    let mut failures = String::new();
    for (rule, guidance) in GATED_RULES {
        let offenders: Vec<&str> =
            findings.iter().filter(|f| f.title.contains(rule)).map(|f| f.title.as_str()).collect();
        if !offenders.is_empty() {
            failures.push_str(&format!("[{rule}] {guidance}; offenders: {offenders:#?}\n"));
        }
    }
    assert!(failures.is_empty(), "rust-quality gates failed:\n{failures}");
}
