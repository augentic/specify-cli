//! Closed source- and target-adapter operation enums.
//!
//! [`SourceOperation`] (`extract | survey`) and [`TargetOperation`]
//! (`shape | build | merge`) are the typed `briefs.keys()` carried by
//! the axis-specific manifest structs in `core.rs`. Living together in
//! this module keeps the source/target operation pair symmetric and
//! gives the cache layer a stable import path (`cache.rs` re-exports
//! [`SourceOperation`] for its `CacheIndexEntry.operation` field so
//! existing cache consumers reach for the type via the cache surface
//! they already import from).
//!
//! Wire format is kebab-case on both sides (`extract | survey` /
//! `shape | build | merge`) — the [`Serialize`] / [`Deserialize`]
//! derives, the [`strum::Display`] impl, and the [`strum::EnumString`]
//! impl all share the same `kebab-case` rule, so the YAML manifest
//! key, the slice outcome `phase` field, the cache index `operation`
//! field, and any `parse::<TargetOperation>()` call all agree on a
//! single wire spelling.
//!
//! pre-2.0 outcome stamps used the legacy `define | build | merge`
//! target set — readers of those archived files must migrate before
//! upgrading; the closed enum here will reject `define` at parse time
//! with a clear error.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use strum::EnumString;

/// Closed source-adapter operation set (`extract | survey`).
///
/// Source adapters declare exactly these two operations per
/// workflow §Source adapter contract. The enum is the typed
/// `briefs.keys()` carried by [`crate::adapter::SourceAdapter`]
/// (parsed out of `adapters/sources/<name>/adapter.yaml` at load
/// time) and the discriminant stamped onto every cache index row at
/// `.specify/.cache/extractions/<adapter>/index.jsonl` so
/// `specify source resolve --explain` can attribute hits and misses
/// (see [`crate::adapter::cache::CacheIndexEntry::operation`]).
///
/// Variants declared in kebab-alphabetical order so `BTreeMap`
/// iteration matches the wire envelope.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    EnumString,
    strum::Display,
    ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SourceOperation {
    /// Slice-time evidence extraction.
    Extract,
    /// Plan-time lead discovery.
    Survey,
}

impl SourceOperation {
    /// Default cached-artifact filename per operation:
    /// `evidence.yaml` for `extract`, `lead-set.md` for `survey`.
    #[must_use]
    pub const fn artifact_name(self) -> &'static str {
        match self {
            Self::Extract => "evidence.yaml",
            Self::Survey => "lead-set.md",
        }
    }
}

/// Closed target-adapter operation set (`shape | build | merge`).
///
/// Target adapters declare exactly these three operations per
/// workflow §Target adapter contract. The enum is the typed
/// `briefs.keys()` carried by [`crate::adapter::TargetAdapter`]
/// (parsed out of `adapters/targets/<name>/adapter.yaml` at load
/// time) and the discriminant stamped into per-slice outcomes
/// (`<slice_dir>/.metadata.yaml.outcome.phase`).
///
/// Replaces the pre-2.0 `Phase { Define, Build, Merge }` — the
/// 1.x define phase has no source/target adapter split counterpart (refine-time artifacts
/// are synthesised by core, not produced by an operation), so the
/// enum collapses to the three target operations.
///
/// Variants declared in kebab-alphabetical order so `BTreeMap`
/// iteration matches the wire envelope.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    EnumString,
    strum::Display,
    ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum TargetOperation {
    /// Build — implementation, driven by `/spec:build`.
    Build,
    /// Merge — landing gate, driven by `/spec:merge`.
    Merge,
    /// Shape — synthesis-time guidance read by core during `/spec:refine`.
    Shape,
}

#[cfg(test)]
mod tests;
