//! On-disk representation of `plan.yaml` and the in-memory [`Plan`]
//! state machine that wraps it. [`Plan::transition`] is the only path
//! that mutates `Entry::status`; see `rfcs/rfc-2-execution.md`.

pub(crate) mod amend;
pub(crate) mod archive;
pub(crate) mod create;
pub(crate) mod io;
pub(crate) mod model;
pub(crate) mod next;
pub(crate) mod transitions;
pub(crate) mod validate;

#[cfg(test)]
mod test_support;

pub use model::{Entry, EntryPatch, Finding, Patch, Plan, Severity, Status};
