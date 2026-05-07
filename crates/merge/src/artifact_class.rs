//! Generic artefact-class slice consumed by [`crate::preview_change`],
//! [`crate::merge_change`], and [`crate::conflict_check`].
//!
//! Per RFC-13 invariant #3 ("concern-specific behaviour leaves core"),
//! the merge engine no longer hard-codes per-domain directory names or
//! per-name strategies. The caller — currently the omnia-default
//! synthesiser in `src/commands/change.rs` — supplies an ordered slice
//! of [`ArtifactClass`] values describing every mutable artefact class
//! that participates in a slice's merge. The engine dispatches on
//! [`MergeStrategy`] and never matches on [`ArtifactClass::name`].
//!
//! Phase 4.1 of RFC-13 will move the omnia-default slice into the
//! capability manifest. Until then, the slice is built at the call
//! site and passed in.
//!
//! See `rfcs/rfc-13-extensibility.md` §"Domain behavior is encoded in
//! Rust" and `rfcs/rfc-13-plan.md` chunk 2.8 for the surrounding
//! design.

use std::path::PathBuf;

/// One mutable artefact class that participates in a slice's merge.
///
/// Each class carries the staged location (under the slice / change
/// directory), the baseline location (relative to the project root),
/// and the [`MergeStrategy`] used to promote staged content into the
/// baseline.
///
/// The [`ArtifactClass::name`] field is for diagnostic output only.
/// The merge engine MUST NOT branch on it; promotion behaviour is
/// driven by [`ArtifactClass::strategy`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactClass {
    /// Identifier from the capability or call site (e.g. `"specs"` or
    /// `"contracts"` for the omnia-default synthesiser). Used purely
    /// for diagnostics and the merge-summary string. The engine never
    /// branches on this field.
    pub name: String,
    /// Where the slice stages this class. Absolute path — typically a
    /// child of the change directory but the engine treats it as an
    /// opaque location.
    pub staged_dir: PathBuf,
    /// Where the baseline lives. Absolute path — typically rooted at
    /// the project root but, again, opaque to the engine.
    pub baseline_dir: PathBuf,
    /// How staged content is promoted into the baseline.
    pub strategy: MergeStrategy,
}

/// Strategy for promoting an [`ArtifactClass`]'s staged content into
/// its baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeStrategy {
    /// 3-way merge of textual content. The engine scans
    /// `<staged_dir>/<name>/spec.md` files (one per spec name) and
    /// merges each delta into the corresponding baseline file at
    /// `<baseline_dir>/<name>/spec.md`. Today's `omnia` "specs"
    /// behaviour. Also pulls in a top-level `composition.yaml` from
    /// the change directory when present (omnia / vectis convention;
    /// stays here for chunk 2.8 and is revisited in Phase 4.1).
    ThreeWayMerge,
    /// Whole-file replacement. The engine walks `<staged_dir>`
    /// recursively and copies each file to the corresponding path
    /// under `<baseline_dir>`, overwriting any existing baseline file.
    /// Today's `omnia` "contracts" behaviour.
    OpaqueReplace,
}
