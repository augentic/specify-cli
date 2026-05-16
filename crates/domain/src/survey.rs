//! Source-survey DTOs, validators, detector contract, registry, and
//! merge helper. See RFC-20 §"Artifacts" and §"Detector Contract".

pub mod detector;
pub mod detectors;
pub mod merge;
pub mod registry;
pub mod sources;

mod dto;
mod validate;

pub use detector::{Detector, DetectorError, DetectorInput, DetectorOutput, Language};
pub use dto::{MetadataDocument, Surface, SurfaceKind, SurfacesDocument};
pub use merge::merge_detector_outputs;
pub use registry::DetectorRegistry;
pub use sources::SourcesFile;
pub use validate::{validate_metadata, validate_surfaces};
