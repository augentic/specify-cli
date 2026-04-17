//! Deterministic delta-merge engine (replaces `merge-specs.py`).
//!
//! Public surface per RFC-1 §`merge.rs`:
//!
//! - [`merge`] — pure in-memory merge of one delta into one (optional) baseline.
//! - [`validate_baseline`] — post-merge coherence checks, ported from the
//!   Python `validate_baseline` (preserves one documented regex quirk).
//! - [`merge_change`] — transactional multi-spec merge + archive that consumes
//!   [`specify_change::ChangeMetadata`], discovers delta specs through
//!   [`specify_schema::PipelineView`], and moves the change directory under
//!   `archive/` once every merge + validation succeeds.
//!
//! Parity with `scripts/legacy/merge-specs.py` is the
//! design goal for [`merge`] and [`validate_baseline`] — see
//! `tests/fixtures/parity/` for the ground-truth outputs the unit tests
//! compare against.

mod change;
mod merge;
mod validate;

pub use change::merge_change;
pub use merge::{MergeOperation, MergeResult, merge};
pub use validate::validate_baseline;
