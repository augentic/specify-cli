//! On-disk representation of `plan.yaml` and the in-memory [`Plan`]
//! state machine that wraps it. [`Plan::transition`] is the only path
//! that mutates `Entry::status`; see `rfcs/rfc-2-execution.md`.

pub mod amend;
pub mod archive;
pub mod create;
pub mod io;
pub mod model;
pub mod next;
pub mod transitions;
pub mod validate;

#[cfg(test)]
mod test_support;

pub use model::{Entry, EntryPatch, Finding, Patch, Plan, Severity, Status};
