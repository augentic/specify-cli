//! Source-survey DTOs, validators, sources file, and ingest pipeline.
//! See RFC-20 §"Artifacts" and §"CLI Verb".

pub mod ingest;
pub mod sources;

mod dto;
mod validate;

pub use dto::{MetadataDocument, Surface, SurfaceKind, SurfacesDocument};
pub use ingest::{IngestInputs, IngestOutcome, ingest};
pub use sources::SourcesFile;
pub use validate::{validate_metadata, validate_surfaces};
