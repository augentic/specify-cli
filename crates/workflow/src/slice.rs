//! Slice `.metadata.yaml`, lifecycle, and naming.
//!
//! Verb-level filesystem operations live in [`actions`].

pub mod actions;
pub mod lifecycle;
pub mod metadata;
pub mod model;
pub mod outcome;
pub mod provenance;

pub use actions::{CreateIfExists, Created, Overlap};
pub use lifecycle::LifecycleStatus;
pub use metadata::{Outcome, SLICES_DIR_NAME, SliceMetadata, SpecKind, TouchedSpec};
pub use model::SliceModel;
pub use outcome::Kind as OutcomeKind;

pub use crate::adapter::TargetOperation;
