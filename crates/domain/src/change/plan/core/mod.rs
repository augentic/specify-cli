//! On-disk representation of `plan.yaml` (at the repo root) and the
//! in-memory [`Plan`] state machine that wraps it.
//!
//! See `rfcs/rfc-2-execution.md` §"Library Implementation" for the
//! canonical type surface and §"The Plan" for the reference YAML
//! fixture exercised by the round-trip tests.
//!
//! # Module layout
//!
//! - [`model`] — types only (`Plan`, `Entry`, `EntryPatch`, `Status`,
//!   `Severity`, `Finding`).
//! - [`io`] — `Plan::load` / `Plan::save` against the on-disk YAML.
//! - [`transitions`] — `Status::can_transition_to` /
//!   `Status::transition`, plus the single-writer `Plan::transition`.
//! - [`amend`] — non-status entry edits via `Plan::amend`.
//! - [`create`] — `Plan::init` (empty-plan scaffold) and
//!   `Plan::create` (single-entry append).
//! - [`validate`] — `Plan::validate` and the per-check helpers.
//! - [`next`] — `Plan::next_eligible` (single-step scheduler) and
//!   `Plan::topological_order` (full dependency-respecting order).
//! - [`archive`] — `Plan::archive` filesystem move into the archive tree.
//!
//! # Single-writer invariant for `Entry::status`
//!
//! The only path that mutates an existing [`Entry::status`] is
//! [`Plan::transition`]. This is not just a convention — it's enforced
//! by the shape of the API:
//!
//!   - [`Plan::create`] appends a new entry and forces its `status` to
//!     [`Status::Pending`]; any other value the caller supplied is
//!     silently overwritten and `status_reason` is cleared.
//!   - [`Plan::amend`] takes an [`EntryPatch`] which structurally
//!     has no `status` (or `status_reason`) field — a type-system
//!     guarantee that `amend` cannot mutate lifecycle state.
//!   - [`Plan::transition`] delegates to [`Status::transition`]
//!     for edge-legality and is the only place that writes
//!     `entry.status` or `entry.status_reason`.

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
