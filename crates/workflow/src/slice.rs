//! Slice `.metadata.yaml`, lifecycle, and naming.
//!
//! Verb-level filesystem operations live in [`actions`].

pub mod actions;
pub mod lifecycle;
pub mod metadata;
pub mod model;
pub mod outcome;
pub mod provenance;
pub mod synthesis;

pub use actions::{CreateIfExists, Created, Overlap};
pub use lifecycle::LifecycleStatus;
pub use metadata::{Outcome, SLICES_DIR_NAME, SliceMetadata, SpecKind, TouchedSpec};
pub use model::SliceModel;
pub use outcome::Kind as OutcomeKind;
pub use synthesis::authority::{Agreement, ClaimRef, Resolution, resolve};
pub use synthesis::project::{ProjectionHeader, project};
pub use synthesis::render::{
    ExpectedRequirement, RenderedSpec, expected_provenance_lines, render_spec_files,
};
pub use synthesis::wire::{
    SynthesisArtifacts, SynthesisInputs, SynthesisResponse, SynthesisSourceInput, SynthesisSpec,
    build_synthesis_inputs,
};

pub use crate::adapter::TargetOperation;
