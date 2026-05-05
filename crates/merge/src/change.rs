//! Transactional multi-spec merge + archive (`merge_change`), plus the
//! no-write `preview_change` variant and the `conflict_check` baseline
//! drift detector.
//!
//! Everything is computed in memory first. We only touch the filesystem
//! after every delta has merged cleanly *and* every merged baseline has
//! passed [`crate::validate_baseline`]. On success `merge_change`:
//!
//!   1. Writes each merged spec baseline under `specs_dir` and contract
//!      baselines under `contracts_dir`.
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

use crate::merge::{MergeOperation, MergeResult, merge};
use crate::validate::validate_baseline;

/// Merged spec pair kept in memory by both [`preview_change`] and
/// [`merge_change`]. Public so CLI callers can inspect `baseline_path`
/// when previewing; the merge path additionally uses it to write.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Spec directory name (e.g. `"login"`).
    pub name: String,
    /// Absolute path where the merged baseline will be written.
    pub baseline_path: PathBuf,
    /// In-memory merge result.
    pub result: MergeResult,
}

/// Complete preview of a change merge: spec merges + contract changes.
#[derive(Debug, Clone)]
#[must_use]
pub struct PreviewResult {
    /// Spec merge entries (existing behavior).
    pub specs: Vec<Entry>,
    /// Contract files that will be copied to baseline.
    pub contracts: Vec<ContractPreviewEntry>,
}

/// A contract file discovered in the change directory.
#[derive(Debug, Clone)]
pub struct ContractPreviewEntry {
    /// Path relative to `contracts/` (e.g. `schemas/user.yaml`).
    pub relative_path: String,
    /// Whether this file already exists in the baseline.
    pub action: ContractAction,
}

/// Whether a contract file is new or replaces an existing baseline file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractAction {
    /// New file — no corresponding baseline file exists.
    Added,
    /// Replacement — a baseline file at the same path will be overwritten.
    Replaced,
}

/// One `type: modified` `touched_spec` whose baseline has been modified
/// after the change's `defined_at` timestamp. The plan skill surfaces
/// this list to the human so they can confirm or abort the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineConflict {
    /// Capability (spec directory) name.
    pub capability: String,
    /// RFC 3339 timestamp when the change was defined.
    pub defined_at: String,
    /// Baseline file modification time.
    pub baseline_modified_at: DateTime<Utc>,
}

/// Dry-run of the multi-spec merge.
///
/// Computes every in-memory [`Entry`] plus runs the baseline
/// coherence validator on each merged output, **without** writing
/// baselines, transitioning status, or archiving. Also reports contract
/// files that will be copied.
///
/// Unlike [`merge_change`] this does not gate on
/// `LifecycleStatus::Complete` — the define / build / merge skill pipeline
/// previews while the change is still `building` or `complete` so the
/// human can confirm operations before the merge skill commits.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn preview_change(
    change_dir: &Path, specs_dir: &Path, contracts_dir: &Path,
) -> Result<PreviewResult, Error> {
    let specs = plan_merge(change_dir, specs_dir)?;
    let contracts = preview_contracts(change_dir, contracts_dir)?;
    Ok(PreviewResult { specs, contracts })
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
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn merge_change(
    change_dir: &Path, specs_dir: &Path, contracts_dir: &Path, archive_dir: &Path,
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

    // --- Copy contract files into baseline ----------------------------------

    let change_contracts_dir = change_dir.join("contracts");

    let contract_files = if change_contracts_dir.is_dir() {
        copy_contracts(&change_contracts_dir, contracts_dir)?
    } else {
        vec![]
    };

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
        summary: if contract_files.is_empty() {
            format!("Merged {} spec(s) into baseline", merged.len())
        } else {
            format!(
                "Merged {} spec(s) and {} contract file(s) into baseline",
                merged.len(),
                contract_files.len()
            )
        },
        context: None,
    });
    metadata.save(change_dir)?;

    actions::archive(change_dir, archive_dir, now)
        .map_err(|err| Error::Merge(format!("archive move failed: {err}")))?;

    let mut output: Vec<(String, MergeResult)> =
        merged.into_iter().map(|e| (e.name, e.result)).collect();
    output.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(output)
}

/// Check for baseline drift on `type: modified` touched specs.
///
/// For each `type: modified` `touched_spec`, report whether the baseline
/// under `specs_dir` has been modified after the change's `defined_at`
/// timestamp. Only `Modified` entries participate — a `New` entry has no
/// baseline to drift from.
///
/// Returns an empty `Vec` when nothing is stale, the change has no
/// `touched_specs`, or `defined_at` is missing (in which case the call
/// is a silent no-op — the merge skill should refuse to proceed until
/// define has run).
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn conflict_check(
    change_dir: &Path, specs_dir: &Path, contracts_dir: &Path,
) -> Result<Vec<BaselineConflict>, Error> {
    let metadata = ChangeMetadata::load(change_dir)?;
    let Some(defined_raw) = metadata.defined_at.as_deref() else {
        return Ok(Vec::new());
    };
    let defined_at = parse_rfc3339(defined_raw)
        .map_err(|err| Error::Merge(format!("cannot parse defined_at `{defined_raw}`: {err}")))?;

    let mut conflicts: Vec<BaselineConflict> = Vec::new();
    for touched in &metadata.touched_specs {
        if touched.kind != SpecType::Modified {
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
    // Check composition baseline for drift
    let composition_delta = change_dir.join("composition.yaml");
    if composition_delta.is_file() {
        let comp_baseline = specs_dir.join("composition.yaml");
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

    // Check contract baseline for drift
    let change_contracts_dir = change_dir.join("contracts");
    if change_contracts_dir.is_dir() {
        check_contract_drift(
            &change_contracts_dir,
            &change_contracts_dir,
            contracts_dir,
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
/// `defined_at`. Files that exist only in the change (not yet in baseline)
/// are skipped — they represent new contracts, not drifted ones.
fn check_contract_drift(
    base: &Path, current: &Path, baseline_dir: &Path, defined_raw: &str, defined_at: DateTime<Utc>,
    conflicts: &mut Vec<BaselineConflict>,
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
            check_contract_drift(base, &path, baseline_dir, defined_raw, defined_at, conflicts)?;
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
                    capability: format!("contracts/{}", relative.to_string_lossy()),
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
/// under `<change_dir>/specs/*/spec.md`. Shared by `preview_change`
/// and `merge_change`.
#[allow(clippy::too_many_lines)]
fn plan_merge(change_dir: &Path, specs_dir: &Path) -> Result<Vec<Entry>, Error> {
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

    let mut merged: Vec<Entry> = Vec::with_capacity(delta_specs.len());
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
            if let specify_capability::ValidationResult::Fail { detail, .. } = vr {
                aborts.push(format!("{}: {detail}", spec.spec_name));
            }
        }

        merged.push(Entry {
            name: spec.spec_name,
            baseline_path: spec.baseline_path,
            result,
        });
    }

    // --- Composition delta (if present) ------------------------------------
    let composition_delta_path = change_dir.join("composition.yaml");
    if composition_delta_path.is_file() {
        let delta_text = fs::read_to_string(&composition_delta_path).map_err(|err| {
            Error::Merge(format!(
                "failed to read composition delta {}: {err}",
                composition_delta_path.display()
            ))
        })?;

        let baseline_path = specs_dir.join("composition.yaml");
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
                            crate::composition::MergeOp::Added { slug } => MergeOperation::Added {
                                id: slug.clone(),
                                name: slug.clone(),
                            },
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
                            crate::composition::MergeOp::CreatedBaseline { screen_count } => {
                                MergeOperation::CreatedBaseline {
                                    requirement_count: *screen_count,
                                }
                            }
                        })
                        .collect(),
                };
                merged.push(Entry {
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

    if !aborts.is_empty() {
        return Err(Error::Merge(aborts.join("\n")));
    }

    merged.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(merged)
}

fn preview_contracts(
    change_dir: &Path, baseline_contracts_dir: &Path,
) -> Result<Vec<ContractPreviewEntry>, Error> {
    let contracts_dir = change_dir.join("contracts");
    if !contracts_dir.is_dir() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    collect_contract_entries(&contracts_dir, &contracts_dir, baseline_contracts_dir, &mut entries)?;
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(entries)
}

fn collect_contract_entries(
    base: &Path, current: &Path, baseline_dir: &Path, entries: &mut Vec<ContractPreviewEntry>,
) -> Result<(), Error> {
    for entry in fs::read_dir(current)
        .map_err(|err| Error::Merge(format!("failed to read {}: {err}", current.display())))?
    {
        let entry = entry.map_err(|err| Error::Merge(format!("dir entry error: {err}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect_contract_entries(base, &path, baseline_dir, entries)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .map_err(|err| Error::Merge(format!("path prefix error: {err}")))?;
            let baseline_path = baseline_dir.join(relative);
            let action = if baseline_path.is_file() {
                ContractAction::Replaced
            } else {
                ContractAction::Added
            };
            entries.push(ContractPreviewEntry {
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

// ---------------------------------------------------------------------------
// Contract file copying
// ---------------------------------------------------------------------------

/// Recursively copy all files from `src` into `dest`, preserving the
/// relative directory structure. Existing files at the same relative
/// path are replaced (opaque whole-file replacement, not delta-merge).
/// Returns the list of relative paths that were copied.
fn copy_contracts(src: &Path, dest: &Path) -> Result<Vec<String>, Error> {
    let mut copied = Vec::new();
    copy_contracts_recursive(src, dest, src, &mut copied)?;
    Ok(copied)
}

fn copy_contracts_recursive(
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
            copy_contracts_recursive(base, dest_base, &path, copied)?;
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

// Archive move semantics live in `specify_change::actions::archive`; both
// `specify change archive` and `merge_change` route through that helper
// so the cross-device-safe `rename → copy-then-remove` fallback has a
// single implementation.
