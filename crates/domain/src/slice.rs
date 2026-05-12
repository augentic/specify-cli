//! Slice `.metadata.yaml`, lifecycle, journal, and naming.
//!
//! Verb-level filesystem operations live in [`actions`].

pub mod actions;
pub mod atomic;
pub mod journal;
pub mod lifecycle;
pub mod metadata;
pub mod outcome;

pub use actions::{CreateIfExists, Created, Overlap};
pub use journal::{EntryKind, Journal, JournalEntry};
pub use lifecycle::LifecycleStatus;
pub use metadata::{
    METADATA_VERSION, Outcome, SLICES_DIR_NAME, SliceMetadata, SpecKind, TouchedSpec,
};
pub use outcome::Kind as OutcomeKind;

pub use crate::capability::Phase;
