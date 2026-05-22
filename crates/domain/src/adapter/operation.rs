//! Target-adapter operation discriminator stamped into per-slice
//! outcomes (`<slice_dir>/.metadata.yaml.outcome.phase`).
//!
//! Replaces the pre-RFC-25 `Phase { Define, Build, Merge }` — the
//! 1.x define phase has no RFC-25 counterpart (refine-time artifacts
//! are synthesised by core, not produced by an operation), so the
//! enum collapses to the three target operations.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Target-adapter operation. Matches the closed `operations[]` set
/// for target adapters declared in `targets/<name>/adapter.yaml`.
///
/// Wire format is kebab-case (`shape | build | merge`), persisted in
/// slice outcome metadata. Pre-RFC-25 outcome stamps used the legacy
/// `define | build | merge` set — readers of those archived files
/// must migrate before upgrading.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display, ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Operation {
    /// Shape — synthesis-time guidance read by core during `/spec:refine`.
    Shape,
    /// Build — implementation, driven by `/spec:build`.
    Build,
    /// Merge — landing gate, driven by `/spec:merge`.
    Merge,
}
