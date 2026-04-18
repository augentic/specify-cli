//! Deterministic delta-merge engine (replaces the archived Python
//! reference implementation; see `tests/fixtures/parity/` for the
//! frozen regression fixtures).
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
//! Parity with the archived Python reference is the
//! design goal for [`merge`] and [`validate_baseline`] — see
//! `tests/fixtures/parity/` for the frozen regression fixtures the unit
//! tests compare against.

mod change;
mod merge;
mod validate;

pub use change::{BaselineConflict, MergeEntry, conflict_check, merge_change, preview_change};
pub use merge::{MergeOperation, MergeResult, merge};
pub use validate::validate_baseline;
