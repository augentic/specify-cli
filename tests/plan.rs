//! Integration tests for the `specify plan *` surface: the executable
//! plan schema, the CLI orchestration verbs, and the fan-in/fan-out
//! reconciliation. CLI submodules live under `plan_orchestrate/`; the
//! pure-schema and fan-in/out suites live under `plan/`. Shared helpers
//! live in [`common`]; the orchestration submodules pull their shared
//! surface in via [`support`].

mod common;

#[path = "plan_orchestrate/support.rs"]
mod support;

#[path = "plan/schema.rs"]
mod schema;

#[path = "plan/fan_in_fan_out.rs"]
mod fan_in_fan_out;

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
