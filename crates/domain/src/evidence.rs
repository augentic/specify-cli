//! RFC-25 source-adapter Evidence shapes.
//!
//! Per-source `extract` output, persisted at
//! `.specify/slices/<slice>/evidence/<source-key>.yaml` and validated
//! against `schemas/evidence.schema.json`. The module collects the
//! claim-kind newtypes (one per closed enum value) and the optional
//! RFC-27 §D2 per-kind authority override map.

pub mod authority;
pub mod claim;

pub use authority::{AuthorityClass, AuthorityOverrides, ClaimKind};
pub use claim::ExampleClaim;
