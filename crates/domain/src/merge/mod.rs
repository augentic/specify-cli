//! Deterministic delta-merge engine (replaces the archived Python
//! reference implementation; see `tests/fixtures/parity/` for the
//! frozen regression fixtures).
//!
//! Public surface:
//!
//! - [`merge`] — pure in-memory merge of one delta into one (optional) baseline.
//! - [`validate_baseline`] — post-merge coherence checks, ported from the
//!   Python `validate_baseline` (preserves one documented regex quirk).
//! - [`slice::commit`] — transactional multi-class merge + archive that
//!   consumes [`crate::slice::SliceMetadata`] plus a caller-supplied
//!   `&[ArtifactClass]` slice; discovers per-class staged content,
//!   promotes it through the class's [`MergeStrategy`], and moves the
//!   slice directory under `archive/` once every merge + validation
//!   succeeds.
//!
//! Parity with the archived Python reference is the design goal for
//! [`merge`] and [`validate_baseline`] — see `tests/fixtures/parity/`
//! for the frozen regression fixtures the unit tests compare against.
//!
//! Concern-specific behaviour is kept out of core: the engine is
//! name-agnostic and never matches on `class.name`. Promotion behaviour
//! is driven by [`MergeStrategy`]; the per-name vocabulary (`specs`,
//! `contracts`, …) is supplied by the caller.

mod artifact_class;
pub mod composition;
mod merge;
pub mod slice;
mod validate;

pub use artifact_class::{ArtifactClass, MergeStrategy};
pub use merge::{MergeOperation, MergeResult, merge};
pub use slice::{
    BaselineConflict, MergePreviewEntry, OpaqueAction, OpaquePreviewEntry, PreviewResult,
    conflict_check,
};
pub use validate::validate_baseline;
