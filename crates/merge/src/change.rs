//! Transactional multi-spec merge + archive (`merge_change`), plus the
//! no-write `preview_change` variant and the `conflict_check` baseline
//! drift detector.
//!
//! Everything is computed in memory first. We only touch the filesystem
//! after every delta has merged cleanly *and* every merged baseline has
//! passed [`crate::validate_baseline`]. On success `merge_change`:
//!
//!   1. Writes each merged baseline under `specs_dir`.
//!   2. Flips `.metadata.yaml.status` from `Complete` to `Merged`.
//!   3. Moves the change directory under `archive_dir` as
//!      `YYYY-MM-DD-<change-name>/` via `specify_change::actions::archive`.
//!
//! Any failure before step 1 returns `Err` with the filesystem untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use specify_change::{ChangeMetadata, LifecycleStatus, SpecType, actions, format_rfc3339};
use specify_error::Error;
use specify_schema::{Phase, PipelineView};

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
    let metadata = ChangeMetadata::load(change_dir)?;
    plan_merge(change_dir, specs_dir, &metadata)
}

/// Atomic multi-spec merge plus archive.
///
/// Gates on `LifecycleStatus::Complete`, runs [`preview_change`]'s
/// in-memory plan, writes each merged baseline, transitions status to
/// `Merged` with `merged_at`/`completed_at` timestamps, then archives the
/// change directory via `specify_change::actions::archive`.
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

    let merged = plan_merge(change_dir, specs_dir, &metadata)?;

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

/// Compute the in-memory merge plan for every delta spec discovered via
/// the merge-phase brief's `generates` glob. Shared by `preview_change`
/// and `merge_change`.
fn plan_merge(
    change_dir: &Path, specs_dir: &Path, metadata: &ChangeMetadata,
) -> Result<Vec<MergeEntry>, Error> {
    // Convention: `<project>/.specify/changes/<name>/`. Three `.parent()`
    // hops land us at `<project>`.
    let project_dir =
        change_dir.parent().and_then(Path::parent).and_then(Path::parent).ok_or_else(|| {
            Error::Merge(format!(
                "cannot resolve project root from change dir {}",
                change_dir.display()
            ))
        })?;

    let pipeline_view = PipelineView::load(&metadata.schema, project_dir).map_err(|err| {
        Error::Merge(format!(
            "failed to load pipeline view for schema `{}`: {err}",
            metadata.schema
        ))
    })?;

    let mut delta_specs: Vec<DeltaSpecRef> = Vec::new();
    for brief in pipeline_view.phase(Phase::Merge) {
        let Some(glob_pattern) = brief.frontmatter.generates.as_deref() else {
            continue;
        };
        let full_glob = change_dir.join(glob_pattern);
        let pattern_str = full_glob
            .to_str()
            .ok_or_else(|| Error::Merge(format!("non-UTF8 glob path: {}", full_glob.display())))?;

        let entries = glob::glob(pattern_str)
            .map_err(|err| Error::Merge(format!("invalid glob `{pattern_str}`: {err}")))?;
        for entry in entries {
            let delta_path =
                entry.map_err(|err| Error::Merge(format!("glob traversal failure: {err}")))?;
            if !delta_path.is_file() {
                continue;
            }
            let spec_name = derive_spec_name(change_dir, &delta_path);
            let baseline_path = specs_dir.join(&spec_name).join("spec.md");
            delta_specs.push(DeltaSpecRef {
                spec_name,
                delta_path,
                baseline_path,
            });
        }
    }

    delta_specs.sort_by(|a, b| a.delta_path.cmp(&b.delta_path));
    delta_specs.dedup_by(|a, b| a.delta_path == b.delta_path);

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

/// Derive the logical spec name from a discovered delta path.
///
/// For the expected shape `<change_dir>/specs/<name>/spec.md` we return
/// `<name>`. For anything weirder we fall back to the file stem so the
/// caller still gets something deterministic to key on.
fn derive_spec_name(change_dir: &Path, delta_path: &Path) -> String {
    let rel = delta_path.strip_prefix(change_dir).unwrap_or(delta_path);
    let components: Vec<&std::ffi::OsStr> = rel.iter().collect();
    if components.len() >= 3
        && components[0] == std::ffi::OsStr::new("specs")
        && components[components.len() - 1] == std::ffi::OsStr::new("spec.md")
        && let Some(name) = components[components.len() - 2].to_str()
    {
        return name.to_string();
    }
    delta_path.file_stem().and_then(|s| s.to_str()).unwrap_or("spec").to_string()
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
