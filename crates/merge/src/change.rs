//! Transactional multi-spec merge + archive (`merge_change`), plus the
//! no-write `preview_change` variant and the `conflict_check` baseline
//! drift detector.
//!
//! Everything is computed in memory first. We only touch the filesystem
//! after every delta has merged cleanly *and* every merged baseline has
//! passed [`crate::validate_baseline`]. On success `merge_change`:
//!
//!   1. Writes each merged baseline under `specs_dir`.
//!   2. Flips `.metadata.yaml.status` from `Complete` to `Merged` and
//!      stamps `PhaseOutcome { phase: Merge, outcome: Success }`.
//!   3. Moves the change directory under `archive_dir` as
//!      `YYYY-MM-DD-<change-name>/` via `specify_change::actions::archive`.
//!
//! Any failure before step 1 returns `Err` with the filesystem untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use specify_change::{
    ChangeMetadata, LifecycleStatus, Outcome, Phase, PhaseOutcome, SpecType, actions,
    format_rfc3339,
};
use specify_error::Error;

use crate::merge::{MergeResult, merge};
use crate::validate::validate_baseline;

/// Merged spec pair kept in memory by both [`preview_change`] and
/// [`merge_change`]. Public so CLI callers can inspect `baseline_path`
/// when previewing; the merge path additionally uses it to write.
#[derive(Debug, Clone)]
pub struct MergeEntry {
    pub spec_name: String,
    pub baseline_path: PathBuf,
    pub result: MergeResult,
}

/// One `type: modified` `touched_spec` whose baseline has been modified
/// after the change's `defined_at` timestamp. The plan skill surfaces
/// this list to the human so they can confirm or abort the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineConflict {
    pub capability: String,
    pub defined_at: String,
    pub baseline_modified_at: DateTime<Utc>,
}

/// Dry-run of the multi-spec merge: computes every in-memory
/// [`MergeEntry`] plus runs the baseline coherence validator on each
/// merged output, **without** writing baselines, transitioning status,
/// or archiving.
///
/// Unlike [`merge_change`] this does not gate on
/// `LifecycleStatus::Complete` — the define / build / merge skill pipeline
/// previews while the change is still `building` or `complete` so the
/// human can confirm operations before the merge skill commits.
pub fn preview_change(change_dir: &Path, specs_dir: &Path) -> Result<Vec<MergeEntry>, Error> {
    plan_merge(change_dir, specs_dir)
}

/// Atomic multi-spec merge plus archive.
///
/// Gates on `LifecycleStatus::Complete`, runs [`preview_change`]'s
/// in-memory plan, writes each merged baseline, transitions status to
/// `Merged` with `merged_at`/`completed_at` timestamps, stamps a
/// `PhaseOutcome { phase: Merge, outcome: Success }` into
/// `.metadata.yaml`, then archives the change directory via
/// `specify_change::actions::archive`.
///
/// The outcome stamp is written atomically with the status transition,
/// before the archive move. This ensures the archived `.metadata.yaml`
/// carries the merge-success outcome so that `/spec:execute` can read
/// it via `specify change outcome` (which falls back to the archive
/// when the active change directory no longer exists).
pub fn merge_change(
    change_dir: &Path, specs_dir: &Path, archive_dir: &Path,
) -> Result<Vec<(String, MergeResult)>, Error> {
    let mut metadata = ChangeMetadata::load(change_dir)?;
    if metadata.status != LifecycleStatus::Complete {
        return Err(Error::Lifecycle {
            expected: "Complete".to_string(),
            found: format!("{:?}", metadata.status),
        });
    }

    let merged = plan_merge(change_dir, specs_dir)?;

    // --- Commit: write baselines ------------------------------------------

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
        summary: format!("Merged {} spec(s) into baseline", merged.len()),
        context: None,
    });
    metadata.save(change_dir)?;

    actions::archive(change_dir, archive_dir, now)
        .map_err(|err| Error::Merge(format!("archive move failed: {err}")))?;

    let mut output: Vec<(String, MergeResult)> =
        merged.into_iter().map(|e| (e.spec_name, e.result)).collect();
    output.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(output)
}

/// For each `type: modified` `touched_spec`, report whether the baseline
/// under `specs_dir` has been modified after the change's `defined_at`
/// timestamp. Only `Modified` entries participate — a `New` entry has no
/// baseline to drift from.
///
/// Returns an empty `Vec` when nothing is stale, the change has no
/// `touched_specs`, or `defined_at` is missing (in which case the call
/// is a silent no-op — the merge skill should refuse to proceed until
/// define has run).
pub fn conflict_check(change_dir: &Path, specs_dir: &Path) -> Result<Vec<BaselineConflict>, Error> {
    let metadata = ChangeMetadata::load(change_dir)?;
    let Some(defined_raw) = metadata.defined_at.as_deref() else {
        return Ok(Vec::new());
    };
    let defined_at = parse_rfc3339(defined_raw)
        .map_err(|err| Error::Merge(format!("cannot parse defined_at `{defined_raw}`: {err}")))?;

    let mut conflicts: Vec<BaselineConflict> = Vec::new();
    for touched in &metadata.touched_specs {
        if touched.spec_type != SpecType::Modified {
            continue;
        }
        let baseline = specs_dir.join(&touched.name).join("spec.md");
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
    conflicts.sort_by(|a, b| a.capability.cmp(&b.capability));
    Ok(conflicts)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Compute the in-memory merge plan for every delta spec discovered
/// under `<change_dir>/specs/*/spec.md`. Shared by `preview_change`
/// and `merge_change`.
fn plan_merge(change_dir: &Path, specs_dir: &Path) -> Result<Vec<MergeEntry>, Error> {
    let mut delta_specs: Vec<DeltaSpecRef> = Vec::new();

    let specs_root = change_dir.join("specs");
    if specs_root.is_dir() {
        for entry in fs::read_dir(&specs_root).map_err(|err| {
            Error::Merge(format!("failed to read {}: {err}", specs_root.display()))
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
            let baseline_path = specs_dir.join(&spec_name).join("spec.md");
            delta_specs.push(DeltaSpecRef {
                spec_name,
                delta_path,
                baseline_path,
            });
        }
    }

    delta_specs.sort_by(|a, b| a.delta_path.cmp(&b.delta_path));

    let mut merged: Vec<MergeEntry> = Vec::with_capacity(delta_specs.len());
    let mut aborts: Vec<String> = Vec::new();

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
            if let specify_schema::ValidationResult::Fail { detail, .. } = vr {
                aborts.push(format!("{}: {detail}", spec.spec_name));
            }
        }

        merged.push(MergeEntry {
            spec_name: spec.spec_name,
            baseline_path: spec.baseline_path,
            result,
        });
    }

    if !aborts.is_empty() {
        return Err(Error::Merge(aborts.join("\n")));
    }

    merged.sort_by(|a, b| a.spec_name.cmp(&b.spec_name));
    Ok(merged)
}

struct DeltaSpecRef {
    spec_name: String,
    delta_path: PathBuf,
    baseline_path: PathBuf,
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

// Archive move semantics live in `specify_change::actions::archive`; both
// `specify change archive` and `merge_change` route through that helper
// so the cross-device-safe `rename → copy-then-remove` fallback has a
// single implementation.
