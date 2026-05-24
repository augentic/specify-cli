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

pub use model::{
    Divergence, Entry, EntryPatch, Finding, Lifecycle, Patch, Plan, Severity,
    SliceAuthorityOverride, SliceSourceBinding, SourceBinding, Status, TargetRef,
    TargetRefParseError,
};
#[cfg(test)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "re-export shared plan test fixtures for sibling modules"
)]
pub(crate) use test_support::{change, change_with_deps, plan_with_changes};
pub use validate::authority_override_orphan_source_keys;
