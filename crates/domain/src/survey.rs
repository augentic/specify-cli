//! Source-survey DTOs, validators, detector contract, registry, merge
//! helper, and ingest pipeline. See RFC-20 §"Artifacts" and §"CLI Verb".
//!
//! The `Detector` trait, [`DetectorRegistry`], and [`merge`] live here as
//! deferred extension points (RFC-20 §"Future mechanical reversion").
//! v1 ships the registry empty; every legacy-code source flows through
//! the agent-driven [`mod@ingest`] pipeline.

pub mod detector;
pub mod ingest;
pub mod merge;
pub mod registry;
pub mod sources;

mod dto;
mod validate;

pub use detector::{Detector, DetectorError, DetectorInput, DetectorOutput, Language};
pub use dto::{MetadataDocument, Surface, SurfaceKind, SurfacesDocument};
pub use ingest::{IngestInputs, IngestOutcome, ingest};
pub use merge::merge_detector_outputs;
pub use registry::DetectorRegistry;
pub use sources::SourcesFile;
pub use validate::{validate_metadata, validate_surfaces};
