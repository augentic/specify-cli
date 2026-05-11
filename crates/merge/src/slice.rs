//! Transactional multi-class merge + archive (`commit`), plus the
//! no-write `preview` variant and the `conflict_check` baseline
//! drift detector.
//!
//! Everything is computed in memory first. We only touch the filesystem
//! after every delta has merged cleanly *and* every merged baseline has
//! passed [`crate::validate_baseline`]. On success `commit`:
//!
//!   1. Writes each merged baseline under the
//!      [`MergeStrategy::ThreeWayMerge`] class's `baseline_dir`, and
//!      copies every staged file under each
//!      [`MergeStrategy::OpaqueReplace`] class's `staged_dir` into its
//!      `baseline_dir`.
//!   2. Flips `.metadata.yaml.status` from `Complete` to `Merged` and
//!      stamps `Outcome { phase: Merge, outcome: Success }`.
//!   3. Moves the slice directory under `archive_dir` as
//!      `YYYY-MM-DD-<slice-name>/` via `specify_slice::actions::archive`.
//!
//! Any failure before step 1 returns `Err` with the filesystem
//! untouched. The engine never branches on a class name; per-class
//! promotion behaviour comes from [`MergeStrategy`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use specify_error::Error;
use specify_slice::{
    LifecycleStatus, Outcome, OutcomeKind, Phase, Rfc3339Stamp, SliceMetadata, SpecKind, actions,
};

use crate::artifact_class::{ArtifactClass, MergeStrategy};
use crate::merge::MergeResult;

mod parse;
mod read;
mod write;

use parse::{parse_rfc3339, system_time_to_utc};
use read::{
    COMPOSITION_FILENAME, check_opaque_drift, first_three_way, plan_three_way, preview_opaque,
};
use write::{build_merge_summary, commit_opaque, write_three_way_baselines};

/// One 3-way merged spec entry kept in memory by both
/// [`preview`] and [`commit`].
///
/// `class_name` carries the originating
/// [`ArtifactClass::name`] so callers can group results without the
/// engine having to know any per-domain vocabulary. `name` is the spec
/// (or composition) identifier within that class.
#[derive(Debug, Clone)]
pub struct MergePreviewEntry {
    /// Originating artefact class name (e.g. `"specs"`).
    pub class_name: String,
    /// Spec/composition name (e.g. `"login"`, `"composition"`).
    pub name: String,
    /// Absolute path where the merged baseline will be written.
    pub baseline_path: PathBuf,
    /// In-memory merge result.
    pub result: MergeResult,
}

/// One opaque-replace file pre-image discovered under a
/// [`MergeStrategy::OpaqueReplace`] class's `staged_dir`.
#[derive(Debug, Clone)]
pub struct OpaquePreviewEntry {
    /// Originating artefact class name (e.g. `"contracts"`).
    pub class_name: String,
    /// Path relative to the class's `staged_dir`
    /// (e.g. `schemas/user.yaml`).
    pub relative_path: String,
    /// Whether this file already exists in the baseline.
    pub action: OpaqueAction,
}

/// Whether an opaque-replace file is new or replaces an existing
/// baseline file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpaqueAction {
    /// New file — no corresponding baseline file exists.
    Added,
    /// Replacement — a baseline file at the same path will be overwritten.
    Replaced,
}

/// Complete preview of a slice merge: 3-way merge entries grouped by
/// class plus opaque-replace pre-images grouped by class.
#[derive(Debug, Clone)]
#[must_use]
pub struct PreviewResult {
    /// 3-way merge entries (one per spec/composition per
    /// `ThreeWayMerge` class). Sorted by `(class_name, name)`.
    pub three_way: Vec<MergePreviewEntry>,
    /// Opaque-replace pre-images (one per file per `OpaqueReplace`
    /// class). Sorted by `(class_name, relative_path)`.
    pub opaque: Vec<OpaquePreviewEntry>,
}

/// One `type: modified` `touched_spec` whose baseline has been modified
/// after the slice's `defined_at` timestamp. The plan skill surfaces
/// this list to the human so they can confirm or abort the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineConflict {
    /// Capability (spec directory) name.
    pub capability: String,
    /// Slice's `defined_at` stamp, copied verbatim from `.metadata.yaml`.
    pub defined_at: String,
    /// Baseline file modification time.
    pub baseline_modified_at: DateTime<Utc>,
}

/// Dry-run of the multi-class merge.
///
/// Computes every in-memory [`MergePreviewEntry`] plus runs the
/// baseline coherence validator on each merged output, **without**
/// writing baselines, transitioning status, or archiving. Also reports
/// every file that would be promoted by an
/// [`MergeStrategy::OpaqueReplace`] class.
///
/// Unlike [`commit`] this does not gate on
/// `LifecycleStatus::Complete` — the define / build / merge skill pipeline
/// previews while the slice is still `building` or `complete` so the
/// human can confirm operations before the merge skill commits.
///
/// # Errors
///
/// - [`Error::Diag { code: "merge-spec-conflicts" }`] aggregating every
///   per-spec merge conflict and post-merge `validate_baseline` failure
///   into a single newline-joined detail string.
/// - [`Error::Filesystem`] (`op = "readdir" | "dir-entry" | "path-prefix"`)
///   for directory-walk failures while scanning the staged trees.
/// - [`Error::Diag { code: "merge-file-type-failed" | "merge-non-utf8-name"
///   | "merge-read-delta-failed" | "merge-read-baseline-failed"
///   | "merge-read-composition-delta-failed"
///   | "merge-read-composition-baseline-failed" }`] for the per-file
///   reads that have no `Error::Filesystem` op equivalent.
/// - Whatever [`Error`] the inner [`crate::merge::merge`] or
///   [`crate::composition::merge`] surfaces, propagated unchanged.
pub fn preview(slice_dir: &Path, classes: &[ArtifactClass]) -> Result<PreviewResult, Error> {
    let three_way = plan_three_way(slice_dir, classes)?;
    let opaque = preview_opaque(classes)?;
    Ok(PreviewResult { three_way, opaque })
}

/// Atomic multi-class merge plus archive.
///
/// Gates on `LifecycleStatus::Complete`, runs [`preview`]'s
/// in-memory plan, writes each merged baseline, transitions status to
/// `Merged` with `merged_at`/`completed_at` timestamps, stamps an
/// `Outcome { phase: Merge, outcome: Success }` into
/// `.metadata.yaml`, then archives the slice directory via
/// `specify_slice::actions::archive`.
///
/// The outcome stamp is written atomically with the status transition,
/// before the archive move. This ensures the archived `.metadata.yaml`
/// carries the merge-success outcome so that `/change:execute` can read
/// it via `specify slice outcome show` (which falls back to the archive
/// when the active slice directory no longer exists).
///
/// `now` records the `merged_at`, `completed_at`, and outcome stamp;
/// dispatchers pass `Utc::now` and tests pin a deterministic value.
///
/// # Errors
///
/// - [`Error::Lifecycle`] when the slice's status is not
///   [`LifecycleStatus::Complete`] on entry, or when the
///   `Complete → Merged` transition is rejected (e.g. terminal-state
///   re-entry).
/// - Every error documented on [`preview`] (the in-memory plan
///   is computed before any writes).
/// - [`Error::Filesystem`] (`op = "mkdir" | "copy"`) when the commit
///   phase fails to create a parent directory or copy an opaque-replace
///   file.
/// - [`Error::Diag { code: "merge-write-baseline-failed" }`] when the
///   commit phase fails to write a merged baseline.
/// - [`Error::Diag { code: "merge-archive-failed" }`] when the archive
///   move fails after metadata has already been flipped.
/// - Whatever atomic-write [`Error`] [`SliceMetadata::save`] surfaces
///   (`Error::Io`, `Error::YamlSer`).
pub fn commit(
    slice_dir: &Path, classes: &[ArtifactClass], archive_dir: &Path, now: DateTime<Utc>,
) -> Result<Vec<MergePreviewEntry>, Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    if metadata.status != LifecycleStatus::Complete {
        return Err(Error::Lifecycle {
            expected: "Complete".to_string(),
            found: format!("{:?}", metadata.status),
        });
    }

    let merged = plan_three_way(slice_dir, classes)?;

    write_three_way_baselines(&merged)?;
    let opaque_counts = commit_opaque(classes)?;

    metadata.status = metadata.status.transition(LifecycleStatus::Merged)?;
    if metadata.completed_at.is_none() {
        metadata.completed_at = Some(Rfc3339Stamp::from(now));
    }
    if metadata.merged_at.is_none() {
        metadata.merged_at = Some(Rfc3339Stamp::from(now));
    }
    metadata.outcome = Some(Outcome {
        phase: Phase::Merge,
        outcome: OutcomeKind::Success,
        at: Rfc3339Stamp::from(now),
        summary: build_merge_summary(&merged, &opaque_counts),
        context: None,
    });
    metadata.save(slice_dir)?;

    actions::archive(slice_dir, archive_dir, now).map_err(|err| Error::Diag {
        code: "merge-archive-failed",
        detail: format!("archive move failed: {err}"),
    })?;

    let mut output: Vec<MergePreviewEntry> = merged;
    output.sort_by(|a, b| {
        (a.class_name.as_str(), a.name.as_str()).cmp(&(b.class_name.as_str(), b.name.as_str()))
    });
    Ok(output)
}

/// Check for baseline drift on the modified `touched_specs` and on
/// every staged opaque-replace file.
///
/// For each `type: modified` `touched_spec`, the check reports whether
/// the corresponding baseline file under each
/// [`MergeStrategy::ThreeWayMerge`] class has been modified after the
/// slice's `defined_at` timestamp. For each staged file under a
/// [`MergeStrategy::OpaqueReplace`] class, the check reports the same
/// drift against the matching baseline file.
///
/// Returns an empty `Vec` when nothing is stale, the slice has no
/// `touched_specs`, or `defined_at` is missing (in which case the call
/// is a silent no-op — the merge skill should refuse to proceed until
/// define has run).
///
/// # Errors
///
/// - [`Error::Diag { code: "merge-defined-at-malformed" }`] when the
///   slice's `defined_at` stamp is present but does not parse as
///   rfc3339.
/// - [`Error::Diag { code: "merge-mtime-pre-epoch" | "merge-mtime-overflow"
///   | "merge-mtime-out-of-range" }`] when a baseline mtime cannot be
///   converted to a UTC `chrono::DateTime`.
/// - [`Error::Io`] when a baseline file's metadata cannot be read for
///   any reason other than `NotFound` (a missing baseline for a
///   `type: modified` entry is treated as a declaration mismatch and
///   silently skipped).
/// - [`Error::Filesystem`] (`op = "readdir" | "dir-entry" | "path-prefix"`)
///   while walking opaque-replace staged trees.
/// - Whatever [`Error`] [`SliceMetadata::load`] surfaces.
pub fn conflict_check(
    slice_dir: &Path, classes: &[ArtifactClass],
) -> Result<Vec<BaselineConflict>, Error> {
    let metadata = SliceMetadata::load(slice_dir)?;
    let Some(defined_raw) = metadata.defined_at.as_deref() else {
        return Ok(Vec::new());
    };
    let defined_at = parse_rfc3339(defined_raw).map_err(|err| Error::Diag {
        code: "merge-defined-at-malformed",
        detail: format!("cannot parse defined_at `{defined_raw}`: {err}"),
    })?;

    let mut conflicts: Vec<BaselineConflict> = Vec::new();

    // Touched-spec drift across every ThreeWayMerge class. Multi-class
    // projects surface drift for each baseline that contains a touched
    // spec name.
    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::ThreeWayMerge)) {
        for touched in &metadata.touched_specs {
            if touched.kind != SpecKind::Modified {
                continue;
            }
            let baseline = class.baseline_dir.join(&touched.name).join("spec.md");
            let meta = match std::fs::metadata(&baseline) {
                Ok(m) => m,
                // A missing baseline for a `type: modified` entry is weird
                // but not a conflict — it's a declaration mismatch for the
                // skill to surface differently. Skip here.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(Error::Io(err)),
            };
            let mtime = system_time_to_utc(meta.modified()?)?;
            if mtime > defined_at {
                conflicts.push(BaselineConflict {
                    capability: touched.name.clone(),
                    defined_at: defined_raw.to_string(),
                    baseline_modified_at: mtime,
                });
            }
        }
    }

    // Composition drift — the convention is exactly one composition
    // delta per slice, promoted into the first ThreeWayMerge class's
    // baseline.
    let composition_delta = slice_dir.join(COMPOSITION_FILENAME);
    if composition_delta.is_file()
        && let Some(class) = first_three_way(classes)
    {
        let comp_baseline = class.baseline_dir.join(COMPOSITION_FILENAME);
        if let Ok(meta) = std::fs::metadata(&comp_baseline) {
            let mtime = system_time_to_utc(meta.modified()?)?;
            if mtime > defined_at {
                conflicts.push(BaselineConflict {
                    capability: "composition".to_string(),
                    defined_at: defined_raw.to_string(),
                    baseline_modified_at: mtime,
                });
            }
        }
    }

    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::OpaqueReplace)) {
        if !class.staged_dir.is_dir() {
            continue;
        }
        check_opaque_drift(
            &class.staged_dir,
            &class.staged_dir,
            &class.baseline_dir,
            &class.name,
            defined_raw,
            defined_at,
            &mut conflicts,
        )?;
    }

    conflicts.sort_by(|a, b| a.capability.cmp(&b.capability));
    Ok(conflicts)
}
