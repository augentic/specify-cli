//! Deterministic delta-merge engine. [`merge`] folds one delta into a
//! baseline; [`validate_baseline`] runs post-merge coherence checks;
//! [`slice::commit`] is the transactional multi-class merge + archive.

mod artifact_class;
pub mod composition;
#[expect(
    clippy::module_inception,
    reason = "preserves the per-concern split inherited from the pre-collapse `specify-merge` crate; rename would cascade across many imports"
)]
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
