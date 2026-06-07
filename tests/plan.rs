//! Integration tests for the `specify plan *` surface: the executable
//! plan schema, the CLI orchestration verbs, and the fan-in/fan-out
//! reconciliation. CLI submodules live under `workflow/`; the
//! pure-schema and fan-in/out suites live under `plan/`. Shared helpers
//! live in [`common`]; the orchestration submodules pull their shared
//! surface in via [`support`].

mod common;

#[path = "workflow/support.rs"]
mod support;

#[path = "plan/schema.rs"]
mod schema;

#[path = "plan/end_to_end.rs"]
mod end_to_end;

#[path = "workflow/validate.rs"]
mod validate;

#[path = "workflow/next.rs"]
mod next;

#[path = "workflow/mutate.rs"]
mod mutate;

#[path = "workflow/source_binding.rs"]
mod source_binding;

#[path = "workflow/transition.rs"]
mod transition;

#[path = "workflow/create.rs"]
mod create;

#[path = "workflow/archive.rs"]
mod archive;

#[path = "workflow/authority.rs"]
mod authority;

#[path = "workflow/propose.rs"]
mod propose;
