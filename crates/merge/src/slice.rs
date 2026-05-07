//! Transactional multi-class merge + archive (`merge_slice`), plus the
//! no-write `preview_slice` variant and the `conflict_check` baseline
//! drift detector.
//!
//! Everything is computed in memory first. We only touch the filesystem
//! after every delta has merged cleanly *and* every merged baseline has
//! passed [`crate::validate_baseline`]. On success `merge_slice`:
//!
//!   1. Writes each merged baseline under the
//!      [`MergeStrategy::ThreeWayMerge`] class's `baseline_dir`, and
//!      copies every staged file under each
//!      [`MergeStrategy::OpaqueReplace`] class's `staged_dir` into its
//!      `baseline_dir`.
//!   2. Flips `.metadata.yaml.status` from `Complete` to `Merged` and
//!      stamps `PhaseOutcome { phase: Merge, outcome: Success }`.
//!   3. Moves the slice directory under `archive_dir` as
//!      `YYYY-MM-DD-<slice-name>/` via `specify_slice::actions::archive`.
//!
//! Any failure before step 1 returns `Err` with the filesystem untouched.
//!
//! RFC-13 §Migration invariant #3 lives here: the engine never branches
//! on a class name; per-class promotion behaviour comes from
//! [`MergeStrategy`]. The omnia-default class slice is synthesised at
//! the binary-side call site (see `src/commands/slice.rs`) and will
//! migrate into the capability manifest in Phase 4.1.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use specify_error::Error;
use specify_slice::{
    LifecycleStatus, Outcome, Phase, PhaseOutcome, SliceMetadata, SpecType, actions, format_rfc3339,
};

use crate::artifact_class::{ArtifactClass, MergeStrategy};
use crate::merge::{MergeOperation, MergeResult, merge};
use crate::validate::validate_baseline;

/// File name for the optional composition delta that lives at the top
/// of a slice directory (alongside `proposal.md` etc.). Promoted into
/// the first [`MergeStrategy::ThreeWayMerge`] class's baseline. Phase
/// 4.1 of RFC-13 moves this convention into the capability manifest.
const COMPOSITION_FILENAME: &str = "composition.yaml";

/// One 3-way merged spec entry kept in memory by both
/// [`preview_slice`] and [`merge_slice`].
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
    /// RFC 3339 timestamp when the slice was defined.
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
/// Unlike [`merge_slice`] this does not gate on
/// `LifecycleStatus::Complete` — the define / build / merge skill pipeline
/// previews while the slice is still `building` or `complete` so the
/// human can confirm operations before the merge skill commits.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn preview_slice(slice_dir: &Path, classes: &[ArtifactClass]) -> Result<PreviewResult, Error> {
    let three_way = plan_three_way(slice_dir, classes)?;
    let opaque = preview_opaque(classes)?;
    Ok(PreviewResult { three_way, opaque })
}

/// Atomic multi-class merge plus archive.
///
/// Gates on `LifecycleStatus::Complete`, runs [`preview_slice`]'s
/// in-memory plan, writes each merged baseline, transitions status to
/// `Merged` with `merged_at`/`completed_at` timestamps, stamps a
/// `PhaseOutcome { phase: Merge, outcome: Success }` into
/// `.metadata.yaml`, then archives the slice directory via
/// `specify_slice::actions::archive`.
///
/// The outcome stamp is written atomically with the status transition,
/// before the archive move. This ensures the archived `.metadata.yaml`
/// carries the merge-success outcome so that `/spec:execute` can read
/// it via `specify slice outcome show` (which falls back to the archive
/// when the active slice directory no longer exists).
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn merge_slice(
    slice_dir: &Path, classes: &[ArtifactClass], archive_dir: &Path,
) -> Result<Vec<MergePreviewEntry>, Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    if metadata.status != LifecycleStatus::Complete {
        return Err(Error::Lifecycle {
            expected: "Complete".to_string(),
            found: format!("{:?}", metadata.status),
        });
    }

    let merged = plan_three_way(slice_dir, classes)?;

    // --- Commit: write 3-way baselines ------------------------------------

    for entry in &merged {
        if let Some(parent) = entry.baseline_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                Error::Merge(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        fs::write(&entry.baseline_path, &entry.result.output).map_err(|err| {
            Error::Merge(format!(
                "failed to write baseline {}: {err}",
                entry.baseline_path.display()
            ))
        })?;
    }

    // --- Commit: copy opaque-replace files into baseline ------------------

    let mut opaque_counts: BTreeMap<String, usize> = BTreeMap::new();
    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::OpaqueReplace)) {
        if !class.staged_dir.is_dir() {
            continue;
        }
        let copied = copy_opaque(&class.staged_dir, &class.baseline_dir)?;
        if !copied.is_empty() {
            opaque_counts.insert(class.name.clone(), copied.len());
        }
    }

    // --- Metadata flip + archive move -------------------------------------

    let now = Utc::now();
    metadata.status = metadata.status.transition(LifecycleStatus::Merged)?;
    if metadata.completed_at.is_none() {
        metadata.completed_at = Some(format_rfc3339(now));
    }
    if metadata.merged_at.is_none() {
        metadata.merged_at = Some(format_rfc3339(now));
    }
    metadata.outcome = Some(PhaseOutcome {
        phase: Phase::Merge,
        outcome: Outcome::Success,
        at: format_rfc3339(now),
        summary: build_merge_summary(&merged, &opaque_counts),
        context: None,
    });
    metadata.save(slice_dir)?;

    actions::archive(slice_dir, archive_dir, now)
        .map_err(|err| Error::Merge(format!("archive move failed: {err}")))?;

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
/// Returns an error if the operation fails.
pub fn conflict_check(
    slice_dir: &Path, classes: &[ArtifactClass],
) -> Result<Vec<BaselineConflict>, Error> {
    let metadata = SliceMetadata::load(slice_dir)?;
    let Some(defined_raw) = metadata.defined_at.as_deref() else {
        return Ok(Vec::new());
    };
    let defined_at = parse_rfc3339(defined_raw)
        .map_err(|err| Error::Merge(format!("cannot parse defined_at `{defined_raw}`: {err}")))?;

    let mut conflicts: Vec<BaselineConflict> = Vec::new();

    // Touched-spec drift across every ThreeWayMerge class. With the
    // current single-class shape this matches pre-2.8 behaviour
    // exactly; multi-class projects (Phase 4.1+) would surface drift
    // for each baseline that contains a touched spec name.
    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::ThreeWayMerge)) {
        for touched in &metadata.touched_specs {
            if touched.kind != SpecType::Modified {
                continue;
            }
            let baseline = class.baseline_dir.join(&touched.name).join("spec.md");
            let meta = match fs::metadata(&baseline) {
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
    // baseline. See `MergeStrategy::ThreeWayMerge` for the rationale.
    let composition_delta = slice_dir.join(COMPOSITION_FILENAME);
    if composition_delta.is_file()
        && let Some(class) = first_three_way(classes)
    {
        let comp_baseline = class.baseline_dir.join(COMPOSITION_FILENAME);
        if let Ok(meta) = fs::metadata(&comp_baseline) {
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

    // Opaque-replace drift across every OpaqueReplace class.
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

/// Recursively walk `current` (rooted at `base`) and check whether each
/// file's counterpart under `baseline_dir` has been modified after
/// `defined_at`. Files that exist only in the staged tree (not yet in
/// baseline) are skipped — they represent new artefacts, not drifted
/// ones.
fn check_opaque_drift(
    base: &Path, current: &Path, baseline_dir: &Path, class_name: &str, defined_raw: &str,
    defined_at: DateTime<Utc>, conflicts: &mut Vec<BaselineConflict>,
) -> Result<(), Error> {
    if !current.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(current)
        .map_err(|err| Error::Merge(format!("failed to read {}: {err}", current.display())))?
    {
        let entry = entry.map_err(|err| Error::Merge(format!("dir entry error: {err}")))?;
        let path = entry.path();
        if path.is_dir() {
            check_opaque_drift(
                base,
                &path,
                baseline_dir,
                class_name,
                defined_raw,
                defined_at,
                conflicts,
            )?;
        } else {
            let relative = path
                .strip_prefix(base)
                .map_err(|err| Error::Merge(format!("path prefix error: {err}")))?;
            let baseline_path = baseline_dir.join(relative);
            let meta = match fs::metadata(&baseline_path) {
                Ok(m) => m,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(Error::Io(err)),
            };
            let mtime = system_time_to_utc(meta.modified()?)?;
            if mtime > defined_at {
                conflicts.push(BaselineConflict {
                    capability: format!("{class_name}/{}", relative.to_string_lossy()),
                    defined_at: defined_raw.to_string(),
                    baseline_modified_at: mtime,
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Compute the in-memory merge plan for every delta spec discovered
/// under each [`MergeStrategy::ThreeWayMerge`] class's `staged_dir`,
/// plus the optional `composition.yaml` delta at the top of the slice
/// directory. Shared by `preview_slice` and `merge_slice`.
#[allow(clippy::too_many_lines)]
fn plan_three_way(
    slice_dir: &Path, classes: &[ArtifactClass],
) -> Result<Vec<MergePreviewEntry>, Error> {
    let mut merged: Vec<MergePreviewEntry> = Vec::new();
    let mut aborts: Vec<String> = Vec::new();
    let mut composition_handled = false;

    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::ThreeWayMerge)) {
        let mut delta_specs: Vec<DeltaSpecRef> = Vec::new();

        if class.staged_dir.is_dir() {
            for entry in fs::read_dir(&class.staged_dir).map_err(|err| {
                Error::Merge(format!("failed to read {}: {err}", class.staged_dir.display()))
            })? {
                let entry = entry.map_err(|err| Error::Merge(format!("dir entry error: {err}")))?;
                let file_type = entry.file_type().map_err(|err| {
                    Error::Merge(format!(
                        "failed to read file type for {}: {err}",
                        entry.path().display()
                    ))
                })?;
                if !file_type.is_dir() {
                    continue;
                }
                let delta_path = entry.path().join("spec.md");
                if !delta_path.is_file() {
                    continue;
                }
                let spec_name = entry
                    .file_name()
                    .to_str()
                    .ok_or_else(|| Error::Merge("non-UTF8 spec directory name".into()))?
                    .to_string();
                let baseline_path = class.baseline_dir.join(&spec_name).join("spec.md");
                delta_specs.push(DeltaSpecRef {
                    spec_name,
                    delta_path,
                    baseline_path,
                });
            }
        }

        delta_specs.sort_by(|a, b| a.delta_path.cmp(&b.delta_path));

        for spec in delta_specs {
            let delta_text = fs::read_to_string(&spec.delta_path).map_err(|err| {
                Error::Merge(format!("failed to read delta {}: {err}", spec.delta_path.display()))
            })?;

            let baseline_text = if spec.baseline_path.is_file() {
                Some(fs::read_to_string(&spec.baseline_path).map_err(|err| {
                    Error::Merge(format!(
                        "failed to read baseline {}: {err}",
                        spec.baseline_path.display()
                    ))
                })?)
            } else {
                None
            };

            let result = match merge(baseline_text.as_deref(), &delta_text) {
                Ok(r) => r,
                Err(Error::Merge(msg)) => {
                    aborts.push(format!("{}: {msg}", spec.spec_name));
                    continue;
                }
                Err(other) => return Err(other),
            };

            for vr in validate_baseline(&result.output, None) {
                if let specify_capability::ValidationResult::Fail { detail, .. } = vr {
                    aborts.push(format!("{}: {detail}", spec.spec_name));
                }
            }

            merged.push(MergePreviewEntry {
                class_name: class.name.clone(),
                name: spec.spec_name,
                baseline_path: spec.baseline_path,
                result,
            });
        }

        // composition.yaml delta — fire once, against the first
        // ThreeWayMerge class. Subsequent ThreeWayMerge classes (if
        // any) skip it; the engine never tries to interpret what
        // composition means for non-omnia/non-vectis domains.
        if !composition_handled {
            composition_handled = true;
            let composition_delta_path = slice_dir.join(COMPOSITION_FILENAME);
            if composition_delta_path.is_file() {
                let delta_text = fs::read_to_string(&composition_delta_path).map_err(|err| {
                    Error::Merge(format!(
                        "failed to read composition delta {}: {err}",
                        composition_delta_path.display()
                    ))
                })?;

                let baseline_path = class.baseline_dir.join(COMPOSITION_FILENAME);
                let baseline_text = if baseline_path.is_file() {
                    Some(fs::read_to_string(&baseline_path).map_err(|err| {
                        Error::Merge(format!(
                            "failed to read composition baseline {}: {err}",
                            baseline_path.display()
                        ))
                    })?)
                } else {
                    None
                };

                match crate::composition::merge_composition(baseline_text.as_deref(), &delta_text) {
                    Ok(comp_result) => {
                        let spec_merge_result = MergeResult {
                            output: comp_result.output,
                            operations: comp_result
                                .operations
                                .iter()
                                .map(|op| match op {
                                    crate::composition::MergeOp::Added { slug } => {
                                        MergeOperation::Added {
                                            id: slug.clone(),
                                            name: slug.clone(),
                                        }
                                    }
                                    crate::composition::MergeOp::Modified { slug } => {
                                        MergeOperation::Modified {
                                            id: slug.clone(),
                                            name: slug.clone(),
                                        }
                                    }
                                    crate::composition::MergeOp::Removed { slug } => {
                                        MergeOperation::Removed {
                                            id: slug.clone(),
                                            name: slug.clone(),
                                        }
                                    }
                                    crate::composition::MergeOp::CreatedBaseline {
                                        screen_count,
                                    } => MergeOperation::CreatedBaseline {
                                        requirement_count: *screen_count,
                                    },
                                })
                                .collect(),
                        };
                        merged.push(MergePreviewEntry {
                            class_name: class.name.clone(),
                            name: "composition".to_string(),
                            baseline_path,
                            result: spec_merge_result,
                        });
                    }
                    Err(Error::Merge(msg)) => {
                        aborts.push(format!("composition: {msg}"));
                    }
                    Err(other) => return Err(other),
                }
            }
        }
    }

    if !aborts.is_empty() {
        return Err(Error::Merge(aborts.join("\n")));
    }

    merged.sort_by(|a, b| {
        (a.class_name.as_str(), a.name.as_str()).cmp(&(b.class_name.as_str(), b.name.as_str()))
    });
    Ok(merged)
}

fn preview_opaque(classes: &[ArtifactClass]) -> Result<Vec<OpaquePreviewEntry>, Error> {
    let mut entries: Vec<OpaquePreviewEntry> = Vec::new();
    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::OpaqueReplace)) {
        if !class.staged_dir.is_dir() {
            continue;
        }
        collect_opaque_entries(
            &class.staged_dir,
            &class.staged_dir,
            &class.baseline_dir,
            &class.name,
            &mut entries,
        )?;
    }
    entries.sort_by(|a, b| {
        (a.class_name.as_str(), a.relative_path.as_str())
            .cmp(&(b.class_name.as_str(), b.relative_path.as_str()))
    });
    Ok(entries)
}

fn collect_opaque_entries(
    base: &Path, current: &Path, baseline_dir: &Path, class_name: &str,
    entries: &mut Vec<OpaquePreviewEntry>,
) -> Result<(), Error> {
    for entry in fs::read_dir(current)
        .map_err(|err| Error::Merge(format!("failed to read {}: {err}", current.display())))?
    {
        let entry = entry.map_err(|err| Error::Merge(format!("dir entry error: {err}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect_opaque_entries(base, &path, baseline_dir, class_name, entries)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .map_err(|err| Error::Merge(format!("path prefix error: {err}")))?;
            let baseline_path = baseline_dir.join(relative);
            let action =
                if baseline_path.is_file() { OpaqueAction::Replaced } else { OpaqueAction::Added };
            entries.push(OpaquePreviewEntry {
                class_name: class_name.to_string(),
                relative_path: relative.to_string_lossy().to_string(),
                action,
            });
        }
    }
    Ok(())
}

struct DeltaSpecRef {
    spec_name: String,
    delta_path: PathBuf,
    baseline_path: PathBuf,
}

fn first_three_way(classes: &[ArtifactClass]) -> Option<&ArtifactClass> {
    classes.iter().find(|c| matches!(c.strategy, MergeStrategy::ThreeWayMerge))
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc))
}

fn system_time_to_utc(t: SystemTime) -> Result<DateTime<Utc>, Error> {
    let duration = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|err| Error::Merge(format!("baseline mtime predates the UNIX epoch: {err}")))?;
    let secs = i64::try_from(duration.as_secs())
        .map_err(|err| Error::Merge(format!("baseline mtime overflow: {err}")))?;
    let nanos = duration.subsec_nanos();
    DateTime::<Utc>::from_timestamp(secs, nanos)
        .ok_or_else(|| Error::Merge("baseline mtime out of range".to_string()))
}

/// Build the operator-facing summary stamped onto the merge phase
/// outcome. Format: `Merged <count> <class>[, <count> <class>]* into
/// baseline`. Empty merges (no work) round-trip as
/// `Merged 0 entries into baseline` so the field is never blank.
fn build_merge_summary(
    three_way: &[MergePreviewEntry], opaque_counts: &BTreeMap<String, usize>,
) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in three_way {
        *counts.entry(entry.class_name.clone()).or_insert(0) += 1;
    }
    for (name, count) in opaque_counts {
        *counts.entry(name.clone()).or_insert(0) += count;
    }
    if counts.is_empty() {
        return "Merged 0 entries into baseline".to_string();
    }
    let parts: Vec<String> =
        counts.iter().map(|(class, count)| format!("{count} {class}")).collect();
    format!("Merged {} into baseline", parts.join(", "))
}

// ---------------------------------------------------------------------------
// Opaque-replace file copying
// ---------------------------------------------------------------------------

/// Recursively copy all files from `src` into `dest`, preserving the
/// relative directory structure. Existing files at the same relative
/// path are replaced (opaque whole-file replacement, not delta-merge).
/// Returns the list of relative paths that were copied.
fn copy_opaque(src: &Path, dest: &Path) -> Result<Vec<String>, Error> {
    let mut copied = Vec::new();
    copy_opaque_recursive(src, dest, src, &mut copied)?;
    Ok(copied)
}

fn copy_opaque_recursive(
    base: &Path, dest_base: &Path, current: &Path, copied: &mut Vec<String>,
) -> Result<(), Error> {
    for entry in fs::read_dir(current)
        .map_err(|err| Error::Merge(format!("failed to read {}: {err}", current.display())))?
    {
        let entry = entry.map_err(|err| Error::Merge(format!("dir entry error: {err}")))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(base)
            .map_err(|err| Error::Merge(format!("path prefix error: {err}")))?;
        let dest_path = dest_base.join(relative);

        if path.is_dir() {
            fs::create_dir_all(&dest_path).map_err(|err| {
                Error::Merge(format!("failed to create {}: {err}", dest_path.display()))
            })?;
            copy_opaque_recursive(base, dest_base, &path, copied)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|err| {
                    Error::Merge(format!("failed to create {}: {err}", parent.display()))
                })?;
            }
            fs::copy(&path, &dest_path).map_err(|err| {
                Error::Merge(format!(
                    "failed to copy {} to {}: {err}",
                    path.display(),
                    dest_path.display()
                ))
            })?;
            copied.push(relative.to_string_lossy().to_string());
        }
    }
    Ok(())
}

// Archive move semantics live in `specify_slice::actions::archive`; both
// `specify slice archive` and `merge_slice` route through that helper
// so the cross-device-safe `rename → copy-then-remove` fallback has a
// single implementation.
