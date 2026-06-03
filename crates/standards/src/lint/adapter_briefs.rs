//! Shared closed adapter-operation sets for the `set-coverage` and
//! `set-eq` hint interpreters (REVIEW.md A10).
//!
//! Both interpreters check an adapter manifest's `briefs.keys()`
//! against the operations its axis must declare. The closed operation
//! sets are held inline here — kept in sync with
//! `specify_workflow::adapter::{SourceOperation, TargetOperation}`
//! (kebab-case wire form) — so the standards-layer crate does not take
//! a workflow-layer dependency, and so the two interpreters share one
//! definition rather than duplicating it.
//!
//! Lives one level above `lint/eval/` so the `every_interpreter_maps_to_kind`
//! parity test (which treats every `lint/eval/<kind>.rs` module as a
//! hint-kind interpreter) does not mistake this shared helper for an
//! orphan interpreter.

use std::collections::BTreeSet;

use crate::lint::AdapterAxis;

/// Closed source-adapter operation set (kebab-case), mirroring
/// `specify_workflow::adapter::SourceOperation`.
pub(crate) const SOURCE_OPERATIONS: &[&str] = &["extract", "survey"];

/// Closed target-adapter operation set (kebab-case), mirroring
/// `specify_workflow::adapter::TargetOperation`.
pub(crate) const TARGET_OPERATIONS: &[&str] = &["build", "merge", "shape"];

/// The operation set a manifest on `axis` must declare in `briefs`.
pub(crate) fn expected_operations(axis: AdapterAxis) -> BTreeSet<&'static str> {
    match axis {
        AdapterAxis::Sources => SOURCE_OPERATIONS.iter().copied().collect(),
        AdapterAxis::Targets => TARGET_OPERATIONS.iter().copied().collect(),
    }
}

/// Kebab-case axis token surfaced in the `set-coverage` / `set-eq`
/// structured evidence payloads.
pub(crate) const fn axis_token(axis: AdapterAxis) -> &'static str {
    match axis {
        AdapterAxis::Sources => "sources",
        AdapterAxis::Targets => "targets",
    }
}
