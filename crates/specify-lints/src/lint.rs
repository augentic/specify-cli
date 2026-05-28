//! Review surface per [DECISIONS.md §"Standards layer split into `specify-lints` and `specify-schema"](../../DECISIONS.md#standards-layer-split-into-specify-lints-and-specify-schema) and §"`WorkspaceModel`".
//!
//! Sibling umbrella to [`crate::rules`]: this module owns the
//! `specrun lint` deterministic review pipeline — `WorkspaceModel` DTOs,
//! the consumer / framework indexer, the hint interpreter, and the
//! diagnostic formatters that `specrun lint` and (later)
//! `specdev lint --format json` share.
//!
//! The submodule shape mirrors the the standards-layer dependency invariant sketch.
//! v1 ships the [`model`] DTO layer; [`index`], [`eval`], and
//! [`diagnostics`] are placeholders filled in by later standards-layer
//! implementation slices.
//!
//! Only the [`model`] surface is re-exported at the umbrella root.
//! [`index`], [`eval`], and [`diagnostics`] stay reachable only by
//! their fully-qualified path so the `rules` (authoring) and
//! `review` (enforcement) surfaces cannot collide.

pub mod diagnostics;
pub mod eval;
pub mod index;
pub mod model;

pub use model::*;
