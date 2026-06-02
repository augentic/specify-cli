//! Integration tests for `specrun plan *` — the top-level verb that
//! orchestrates the executable plan at `plan.yaml` (the executable plan contract).
//!
//! These CLI tests stand up a fresh `.specify/` project via `specify
//! init` (mirroring `tests/slice.rs` / `tests/e2e.rs`), seed
//! `plan.yaml` at the repo root by writing YAML directly to disk, and
//! drive the CLI through `assert_cmd`. JSON shapes are pinned by
//! checked-in fixtures under `tests/fixtures/plan/`; regenerate them
//! with
//! `REGENERATE_GOLDENS=1 cargo test --test plan_orchestrate`.
//!
//! The suite is split across themed submodules under
//! `tests/plan_orchestrate/` (REVIEW.md A13); shared imports, helpers,
//! and plan seeds live in [`support`].

mod common;

#[path = "plan_orchestrate/support.rs"]
mod support;

#[path = "plan_orchestrate/lifecycle.rs"]
mod lifecycle;

#[path = "plan_orchestrate/archive.rs"]
mod archive;

#[path = "plan_orchestrate/authority.rs"]
mod authority;

#[path = "plan_orchestrate/propose.rs"]
mod propose;
