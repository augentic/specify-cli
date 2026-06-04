//! Integration tests for `specify plan *` — the top-level verb that
//! orchestrates the executable plan at `plan.yaml` (the executable plan contract).
//!
//! These CLI tests stand up a fresh `.specify/` project via `specify
//! init` (mirroring `tests/slice.rs` / `tests/e2e.rs`), seed
//! `plan.yaml` at the repo root by writing YAML directly to disk, and
//! drive the CLI through `assert_cmd`. JSON shapes are pinned by
//! checked-in fixtures under `tests/fixtures/plan/`; regenerate them
//! with
//! `REGENERATE_GOLDENS=1 cargo nextest run --test plan_orchestrate`.
//!
//! The suite is split across themed submodules under
//! `tests/plan_orchestrate/`, grouped by `plan` command family; shared
//! imports, helpers, and plan seeds live in [`support`].

mod common;

#[path = "plan_orchestrate/support.rs"]
mod support;

#[path = "plan_orchestrate/validate.rs"]
mod validate;

#[path = "plan_orchestrate/next.rs"]
mod next;

#[path = "plan_orchestrate/mutate.rs"]
mod mutate;

#[path = "plan_orchestrate/source_binding.rs"]
mod source_binding;

#[path = "plan_orchestrate/transition.rs"]
mod transition;

#[path = "plan_orchestrate/create.rs"]
mod create;

#[path = "plan_orchestrate/archive.rs"]
mod archive;

#[path = "plan_orchestrate/authority.rs"]
mod authority;

#[path = "plan_orchestrate/propose.rs"]
mod propose;
