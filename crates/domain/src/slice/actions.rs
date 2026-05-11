//! Verb-level operations on a Specify slice directory.
//!
//! `actions` turns the static lifecycle state machine in the crate root
//! into transactional filesystem operations: creating a fresh slice
//! directory, transitioning its `.metadata.yaml` status with the
//! associated timestamp write, scanning `specs/` for `touched_specs`,
//! detecting overlap against other active slices, and archiving
//! (`archive` / `discard`) into `.specify/archive/YYYY-MM-DD-<name>/`.
//!
//! Every verb is expressed as a free function rather than a struct method
//! so the CLI can dispatch each subcommand with one import per verb. They
//! all round-trip through [`crate::slice::SliceMetadata::save`] for the metadata
//! writes and share the cross-device-safe [`crate::slice::actions::io::move_atomic`]
//! helper for archive moves.

pub mod archive;
pub mod create;
pub mod discard;
pub mod io;
pub mod outcome;
pub mod overlap;
pub mod scan;
pub mod transition;

pub use archive::archive;
pub use create::{CreateIfExists, Created, create, validate_name};
pub use discard::discard;
pub use io::move_atomic;
pub use outcome::stamp_outcome;
pub use overlap::{Overlap, overlap};
pub use scan::{scan_touched, write_touched};
pub use transition::transition;
