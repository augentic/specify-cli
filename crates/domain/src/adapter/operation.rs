//! Closed source- and target-adapter operation enums.
//!
//! [`SourceOperation`] (`enumerate | extract`) and [`TargetOperation`]
//! (`shape | build | merge`) are the typed `briefs.keys()` carried by
//! the axis-specific manifest structs in `core.rs`. Living together in
//! this module keeps the source/target operation pair symmetric and
//! gives the cache layer a stable import path (`cache.rs` re-exports
//! [`SourceOperation`] for its `CacheIndexEntry.operation` field so
//! existing cache consumers reach for the type via the cache surface
//! they already import from).
//!
//! Wire format is kebab-case on both sides (`enumerate | extract` /
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

/// Closed source-adapter operation set (`enumerate | extract`).
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
/// `Ord` is implemented manually so [`std::collections::BTreeMap`]
/// keyed on this enum iterates in kebab-alphabetical order
/// (`enumerate < extract`) — the wire-format guarantee pinned by the
/// `specify source resolve` envelope tests and the cache-index
/// canonicalisation. Variant declaration order tracks the operational
/// pipeline (plan-time → slice-time); the two orderings happen to
/// agree here, but the manual impl keeps the wire-format invariant
/// independent of future variant reshuffles.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
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
    /// Plan-time candidate discovery.
    Enumerate,
    /// Slice-time evidence extraction.
    Extract,
}

impl Ord for SourceOperation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl PartialOrd for SourceOperation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl SourceOperation {
    /// Default cached-artifact filename per operation:
    /// `evidence.yaml` for `extract`, `candidate-set.md` for `enumerate`.
    #[must_use]
    pub const fn artifact_name(self) -> &'static str {
        match self {
            Self::Enumerate => "candidate-set.md",
            Self::Extract => "evidence.yaml",
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
/// 1.x define phase has no RFC-25 counterpart (refine-time artifacts
/// are synthesised by core, not produced by an operation), so the
/// enum collapses to the three target operations.
///
/// `Ord` is implemented manually so [`std::collections::BTreeMap`]
/// keyed on this enum iterates in kebab-alphabetical order
/// (`build < merge < shape`) — the wire-format guarantee pinned by
/// the `specify target resolve` envelope tests. Variant declaration
/// order tracks the operational pipeline (refine-time shape →
/// build-time build → landing-time merge); decoupling the wire
/// iteration order from variant order keeps the envelope stable
/// against any future reshuffles for readability.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
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
    /// Shape — synthesis-time guidance read by core during `/spec:refine`.
    Shape,
    /// Build — implementation, driven by `/spec:build`.
    Build,
    /// Merge — landing gate, driven by `/spec:merge`.
    Merge,
}

impl Ord for TargetOperation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl PartialOrd for TargetOperation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn source_operation_round_trips_kebab_case() {
        assert_eq!(SourceOperation::Enumerate.to_string(), "enumerate");
        assert_eq!(SourceOperation::Extract.to_string(), "extract");
        assert_eq!(
            <SourceOperation as FromStr>::from_str("enumerate"),
            Ok(SourceOperation::Enumerate)
        );
        assert_eq!(<SourceOperation as FromStr>::from_str("extract"), Ok(SourceOperation::Extract));
        <SourceOperation as FromStr>::from_str("shape")
            .expect_err("`shape` is a target op; must not parse as a SourceOperation");
        let json = serde_json::to_string(&SourceOperation::Extract).expect("serialise");
        assert_eq!(json, "\"extract\"");
        let back: SourceOperation = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, SourceOperation::Extract);
    }

    #[test]
    fn target_operation_round_trips_kebab_case() {
        assert_eq!(TargetOperation::Shape.to_string(), "shape");
        assert_eq!(TargetOperation::Build.to_string(), "build");
        assert_eq!(TargetOperation::Merge.to_string(), "merge");
        assert_eq!(<TargetOperation as FromStr>::from_str("shape"), Ok(TargetOperation::Shape));
        assert_eq!(<TargetOperation as FromStr>::from_str("build"), Ok(TargetOperation::Build));
        assert_eq!(<TargetOperation as FromStr>::from_str("merge"), Ok(TargetOperation::Merge));
        <TargetOperation as FromStr>::from_str("define")
            .expect_err("legacy `define` must not parse as a TargetOperation");
        let json = serde_json::to_string(&TargetOperation::Merge).expect("serialise");
        assert_eq!(json, "\"merge\"");
        let back: TargetOperation = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, TargetOperation::Merge);
    }

    #[test]
    fn unknown_operation_rejected_with_clear_error() {
        let err = serde_json::from_str::<SourceOperation>("\"foo\"")
            .expect_err("unknown source operation must fail");
        let detail = err.to_string();
        assert!(detail.contains("foo") || detail.contains("enumerate"), "{detail}");

        let err = serde_json::from_str::<TargetOperation>("\"define\"")
            .expect_err("legacy `define` rejected on the target axis");
        let detail = err.to_string();
        assert!(detail.contains("define") || detail.contains("shape"), "{detail}");
    }
}
