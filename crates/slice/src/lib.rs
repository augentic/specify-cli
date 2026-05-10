//! Slice `.metadata.yaml` document and lifecycle state machine.
//!
//! Exposes [`SliceMetadata`] and the [`LifecycleStatus`] graph between
//! `Defining`, `Defined`, `Building`, `Complete`, `Merged`, `Dropped`.
//! Verb-level operations live in [`actions`].

/// Verb-level operations on a Specify slice directory.
pub mod actions;
/// Crash-safe write helpers shared with `specify-change`.
pub mod atomic;
/// On-disk journal for append-only audit logging.
pub mod journal;
/// Lifecycle state machine.
pub mod lifecycle;
/// `.metadata.yaml` document, version constant, and atomic save/load.
pub mod metadata;
/// Phase-outcome discriminant returned by `/change:execute`.
pub mod outcome;
/// Timestamp newtype.
pub mod timestamp;

pub use actions::{CreateIfExists, CreateOutcome, Overlap, format_rfc3339};
pub use journal::{EntryKind, Journal, JournalEntry};
pub use lifecycle::LifecycleStatus;
pub use metadata::{
    METADATA_VERSION, PhaseOutcome, SLICES_DIR_NAME, SliceMetadata, SpecKind, TouchedSpec,
};
pub use outcome::Outcome;
pub use specify_capability::Phase;
pub use timestamp::Rfc3339Stamp;
