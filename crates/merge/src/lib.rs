//! Deterministic delta-merge engine (replaces the archived Python
//! reference implementation; see `tests/fixtures/parity/` for the
//! frozen regression fixtures).
//!
//! Public surface per RFC-1 §`merge.rs`:
//!
//! - [`merge`] — pure in-memory merge of one delta into one (optional) baseline.
//! - [`validate_baseline`] — post-merge coherence checks, ported from the
//!   Python `validate_baseline` (preserves one documented regex quirk).
//! - [`merge_change`] — transactional multi-class merge + archive that consumes
//!   [`specify_change::ChangeMetadata`] plus a caller-supplied
//!   `&[ArtifactClass]` slice (RFC-13 §"Domain behavior is encoded in
//!   Rust"); discovers per-class staged content, promotes it through the
//!   class's [`MergeStrategy`], and moves the change directory under
//!   `archive/` once every merge + validation succeeds.
//!
//! Parity with the archived Python reference is the design goal for
//! [`merge`] and [`validate_baseline`] — see `tests/fixtures/parity/`
//! for the frozen regression fixtures the unit tests compare against.
//!
//! RFC-13 §Migration invariant #3 — "concern-specific behaviour leaves
//! core" — is operationalised here: the engine is name-agnostic and
//! never matches on `class.name`. Promotion behaviour is driven by
//! [`MergeStrategy`]; the per-name vocabulary (`specs`, `contracts`,
//! …) is supplied by the caller.

mod artifact_class;
mod change;
pub mod composition;
mod merge;
mod validate;

pub use artifact_class::{ArtifactClass, MergeStrategy};
pub use change::{
    BaselineConflict, MergePreviewEntry, OpaqueAction, OpaquePreviewEntry, PreviewResult,
    conflict_check, merge_change, preview_change,
};
pub use merge::{MergeOperation, MergeResult, merge};
pub use validate::validate_baseline;
