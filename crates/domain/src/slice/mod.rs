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

pub use actions::{CreateIfExists, Created, Overlap};
pub use journal::{EntryKind, Journal, JournalEntry};
pub use lifecycle::LifecycleStatus;
pub use metadata::{
    METADATA_VERSION, Outcome, SLICES_DIR_NAME, SliceMetadata, SpecKind, TouchedSpec,
};
pub use outcome::OutcomeKind;
pub use crate::capability::Phase;
