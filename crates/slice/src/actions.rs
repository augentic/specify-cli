//! Verb-level operations on a Specify slice directory.
//!
//! `actions` turns the static lifecycle state machine in the crate root
//! into transactional filesystem operations: creating a fresh slice
//! directory, transitioning its `.metadata.yaml` status with the
//! associated timestamp write, scanning `specs/` for `touched_specs`,
//! detecting overlap against other active slices, and archiving
//! (`archive` / `drop`) into `.specify/archive/YYYY-MM-DD-<name>/`.
//!
//! Every verb is expressed as a free function rather than a struct method
//! so the CLI can dispatch each subcommand with one import per verb. They
//! all round-trip through [`SliceMetadata::save`] for the metadata writes
//! and share the cross-device-safe `move_atomic` helper for archive
//! moves.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use specify_error::{Error, is_kebab};

use crate::{
    LifecycleStatus, Outcome, Phase, PhaseOutcome, Rfc3339Stamp, SliceMetadata, SpecKind,
    TouchedSpec,
};

/// What to do when [`create`] finds an existing directory at the
/// target path.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreateIfExists {
    /// Default. Refuse and return `Error::Diag`.
    Fail,
    /// Reuse the existing directory. The function reloads its
    /// `.metadata.yaml` and returns it without writing. Intended for the
    /// define skill's "continue in-flight slice" flow.
    Continue,
    /// Delete and recreate. Intended for the define skill's "restart"
    /// flow. The caller is expected to have already archived anything it
    /// wants to keep — this branch is destructive.
    Restart,
}

/// Outcome of [`create`], surfacing whether a new directory was written
/// or an existing one was reused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct CreateOutcome {
    /// Path to the slice directory.
    pub dir: PathBuf,
    /// Loaded or freshly-created metadata.
    pub metadata: SliceMetadata,
    /// `true` when the call created a new directory; `false` when an
    /// existing directory was reused (`CreateIfExists::Continue`).
    pub created: bool,
    /// `true` when the call replaced an existing directory
    /// (`CreateIfExists::Restart`).
    pub restarted: bool,
}

/// A capability-level conflict between two active slices both touching
/// the same spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    /// The shared capability name.
    pub capability: String,
    /// Name of the other slice that touches the same capability.
    pub other: String,
    /// How our slice touches the capability.
    pub ours: SpecKind,
    /// How the other slice touches the capability.
    pub theirs: SpecKind,
}

/// Validate a kebab-case slice name.
///
/// Mirrors `schemas/plan/plan.schema.json` `$defs.kebabName.pattern`
/// (`^[a-z0-9]+(-[a-z0-9]+)*$`).
///
/// # Errors
///
/// Returns `Error::InvalidName` if the name is not valid kebab-case.
pub fn validate_name(name: &str) -> Result<(), Error> {
    if is_kebab(name) {
        Ok(())
    } else {
        Err(Error::InvalidName(format!(
            "slice name `{name}` must be kebab-case (lowercase ascii, digits, single hyphens; \
             no leading/trailing/doubled hyphens)"
        )))
    }
}

/// Create `<slices_dir>/<name>/` and seed an initial `.metadata.yaml`.
///
/// - `slices_dir` is expected to be `<project>/.specify/slices/`.
/// - `now` is plumbed in so tests can pin `created_at` deterministically.
///
/// On success returns a [`CreateOutcome`] with the resolved directory and
/// loaded metadata. Behaviour when the directory already exists is
/// governed by `if_exists` — see [`CreateIfExists`].
///
/// # Errors
///
/// Returns an error if the operation fails.
#[expect(
    clippy::similar_names,
    reason = "`slices_dir` and `slice_dir` name distinct concepts (parent dir vs. this slice's dir)."
)]
pub fn create(
    slices_dir: &Path, name: &str, capability: &str, if_exists: CreateIfExists, now: DateTime<Utc>,
) -> Result<CreateOutcome, Error> {
    validate_name(name)?;
    let slice_dir = slices_dir.join(name);
    let metadata_path = SliceMetadata::path(&slice_dir);

    if slice_dir.exists() {
        match if_exists {
            CreateIfExists::Fail => {
                return Err(Error::Diag {
                    code: "slice-already-exists",
                    detail: format!("slice `{name}` already exists at {}", slice_dir.display()),
                });
            }
            CreateIfExists::Continue => {
                if !metadata_path.exists() {
                    return Err(Error::Diag {
                        code: "slice-dir-missing-metadata",
                        detail: format!(
                            "slice dir {} exists but has no .metadata.yaml; refusing to reuse",
                            slice_dir.display()
                        ),
                    });
                }
                let metadata = SliceMetadata::load(&slice_dir)?;
                return Ok(CreateOutcome {
                    dir: slice_dir,
                    metadata,
                    created: false,
                    restarted: false,
                });
            }
            CreateIfExists::Restart => {
                std::fs::remove_dir_all(&slice_dir)?;
            }
        }
    }

    std::fs::create_dir_all(slice_dir.join("specs"))?;
    let metadata = SliceMetadata {
        version: crate::METADATA_VERSION,
        capability: capability.to_string(),
        status: LifecycleStatus::Defining,
        created_at: Some(format_rfc3339(now)),
        defined_at: None,
        build_started_at: None,
        completed_at: None,
        merged_at: None,
        dropped_at: None,
        drop_reason: None,
        touched_specs: Vec::new(),
        outcome: None,
    };
    metadata.save(&slice_dir)?;

    Ok(CreateOutcome {
        dir: slice_dir,
        metadata,
        created: true,
        restarted: matches!(if_exists, CreateIfExists::Restart),
    })
}

/// Transition a slice to `target` status and write the matching timestamp.
///
/// The transition is validated by
/// [`LifecycleStatus::transition`](crate::LifecycleStatus::transition) —
/// illegal edges return `Error::Lifecycle` without touching disk. On
/// success the metadata's `status` is updated, the appropriate
/// `*_at` timestamp is filled in (idempotent: an existing non-`None`
/// timestamp is preserved), and `.metadata.yaml` is rewritten atomically.
///
/// Returns the updated `SliceMetadata`.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn transition(
    slice_dir: &Path, target: LifecycleStatus, now: DateTime<Utc>,
) -> Result<SliceMetadata, Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    metadata.status = metadata.status.transition(target)?;
    let stamp = format_rfc3339(now);
    match target {
        LifecycleStatus::Defining => {
            if metadata.created_at.is_none() {
                metadata.created_at = Some(stamp);
            }
        }
        LifecycleStatus::Defined => {
            if metadata.defined_at.is_none() {
                metadata.defined_at = Some(stamp);
            }
        }
        LifecycleStatus::Building => {
            if metadata.build_started_at.is_none() {
                metadata.build_started_at = Some(stamp);
            }
        }
        LifecycleStatus::Complete => {
            if metadata.completed_at.is_none() {
                metadata.completed_at = Some(stamp);
            }
        }
        LifecycleStatus::Merged => {
            if metadata.merged_at.is_none() {
                metadata.merged_at = Some(stamp);
            }
        }
        LifecycleStatus::Dropped => {
            if metadata.dropped_at.is_none() {
                metadata.dropped_at = Some(stamp);
            }
        }
    }
    metadata.save(slice_dir)?;
    Ok(metadata)
}

/// Scan `<slice_dir>/specs/*` and classify each capability as
/// `new` or `modified` against `<specs_dir>/<name>/spec.md`.
///
/// Returns entries sorted by capability name for stable output. The
/// scan is non-destructive — it does not mutate `.metadata.yaml`. The
/// caller typically follows up with [`write_touched`].
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn scan_touched(slice_dir: &Path, specs_dir: &Path) -> Result<Vec<TouchedSpec>, Error> {
    let specs_root = slice_dir.join("specs");
    if !specs_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<TouchedSpec> = Vec::new();
    for entry in std::fs::read_dir(&specs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // Only classify as touched when a spec.md actually exists; an
        // empty subdirectory is noise left over from init/define work
        // in progress.
        if !entry.path().join("spec.md").is_file() {
            continue;
        }
        let baseline = specs_dir.join(&name).join("spec.md");
        let kind = if baseline.is_file() { SpecKind::Modified } else { SpecKind::New };
        entries.push(TouchedSpec { name, kind });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Overwrite `.metadata.yaml`'s `touched_specs` with `entries`.
///
/// Leaves every other field on the struct untouched, including `status`.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn write_touched(slice_dir: &Path, entries: Vec<TouchedSpec>) -> Result<SliceMetadata, Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    metadata.touched_specs = entries;
    metadata.save(slice_dir)?;
    Ok(metadata)
}

/// Detect overlap between this slice's `touched_specs` and every other
/// active slice's. "Active" means a directory under `slices_dir` that
/// has a `.metadata.yaml` and is not `slice_name` itself.
///
/// Merged and dropped slices still appear on disk until the archive
/// move completes, so we additionally filter by status: only
/// non-terminal statuses participate. Archive directories under
/// `slices_dir` (e.g. `<slices_dir>/archive/...`) are not scanned.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn overlap(slices_dir: &Path, slice_name: &str) -> Result<Vec<Overlap>, Error> {
    let self_dir = slices_dir.join(slice_name);
    let self_meta = SliceMetadata::load(&self_dir)?;
    if self_meta.touched_specs.is_empty() {
        return Ok(Vec::new());
    }

    let mut overlaps: Vec<Overlap> = Vec::new();
    let Ok(entries) = std::fs::read_dir(slices_dir) else {
        return Ok(Vec::new());
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let other_path = entry.path();
        let Some(other_name) = other_path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if other_name == slice_name || other_name == "archive" {
            continue;
        }
        if !SliceMetadata::path(&other_path).exists() {
            continue;
        }
        let other_meta = SliceMetadata::load(&other_path)?;
        if other_meta.status.is_terminal() {
            continue;
        }
        for ours in &self_meta.touched_specs {
            for theirs in &other_meta.touched_specs {
                if ours.name == theirs.name {
                    overlaps.push(Overlap {
                        capability: ours.name.clone(),
                        other: other_name.to_string(),
                        ours: ours.kind,
                        theirs: theirs.kind,
                    });
                }
            }
        }
    }
    overlaps.sort_by(|a, b| a.capability.cmp(&b.capability).then_with(|| a.other.cmp(&b.other)));
    Ok(overlaps)
}

/// Move `slice_dir` to `<archive_dir>/YYYY-MM-DD-<slice-name>/`.
///
/// This is the sole implementation of the archive move semantics; both
/// `specify slice archive` and the `specify slice merge run` success path
/// route through it. Does **not** touch `.metadata.yaml` — the caller is
/// responsible for any status transition before or after.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn archive(
    slice_dir: &Path, archive_dir: &Path, today: DateTime<Utc>,
) -> Result<PathBuf, Error> {
    let slice_name = slice_dir.file_name().and_then(|s| s.to_str()).ok_or_else(|| Error::Diag {
        code: "slice-dir-no-basename",
        detail: format!("slice dir {} has no basename", slice_dir.display()),
    })?;
    let date = today.format("%Y-%m-%d").to_string();
    let target = archive_dir.join(format!("{date}-{slice_name}"));
    std::fs::create_dir_all(archive_dir)?;
    move_atomic(slice_dir, &target)?;
    Ok(target)
}

/// Stamp the outcome of a phase run on `<slice_dir>/.metadata.yaml`.
///
/// Primary writer of [`SliceMetadata::outcome`] for the define and
/// build phases, and for merge failure/deferred outcomes. The merge
/// success path is handled by `specify_merge::slice::commit`, which
/// stamps the outcome atomically before archiving (the archive move
/// removes `slice_dir` from `.specify/slices/`, so a post-merge call
/// to this function would fail with "not found").
///
/// The whole metadata file is rewritten atomically via
/// [`SliceMetadata::save`] so a concurrent reader never sees a
/// half-written file. A new stamp replaces any previous one — history
/// lives in `journal.yaml` (L2.B), not here.
///
/// `now` is plumbed in so tests can pin `at` deterministically; the CLI
/// passes `Utc::now()`.
///
/// Returns the updated [`SliceMetadata`].
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn stamp_outcome(
    slice_dir: &Path, phase: Phase, outcome: Outcome, summary: &str, context: Option<&str>,
    now: DateTime<Utc>,
) -> Result<SliceMetadata, Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    metadata.outcome = Some(PhaseOutcome {
        phase,
        outcome,
        at: format_rfc3339(now),
        summary: summary.to_string(),
        context: context.map(str::to_string),
    });
    metadata.save(slice_dir)?;
    Ok(metadata)
}

/// Transition a slice to `Dropped`, record the optional reason, then
/// archive. Returns the final archive path.
///
/// Valid from any non-terminal lifecycle state. Callers use this for
/// both failure ("dropped because build broke") and deferral ("blocked
/// on a design question") — the plan layer above turns the reason into
/// `failure-reason` or `block-reason`; here it's just free text.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn drop(
    slice_dir: &Path, archive_dir: &Path, reason: Option<&str>, now: DateTime<Utc>,
) -> Result<(SliceMetadata, PathBuf), Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    metadata.status = metadata.status.transition(LifecycleStatus::Dropped)?;
    metadata.dropped_at = Some(format_rfc3339(now));
    if let Some(text) = reason {
        metadata.drop_reason = Some(text.to_string());
    }
    metadata.save(slice_dir)?;
    let target = archive(slice_dir, archive_dir, now)?;
    Ok((metadata, target))
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Canonical ISO 8601 timestamp shape used by every `.specify/*` writer
/// in this crate.
///
/// Pinned to the second-precision `%Y-%m-%dT%H:%M:%SZ` form so every
/// on-disk timestamp matches the regex
/// `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$` — no sub-second component,
/// no offset suffix, no `+00:00` form. `chrono::DateTime::to_rfc3339`
/// is deliberately NOT used because it varies by input (it may
/// include microseconds on some paths), which would break golden
/// fixtures that pin the full shape.
///
/// Re-exported at the crate root so the `specify` binary can route
/// its own timestamp writers (e.g. `slice journal append`) through
/// the same helper.
#[must_use]
pub fn format_rfc3339(now: DateTime<Utc>) -> Rfc3339Stamp {
    Rfc3339Stamp::new(now.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// `EXDEV` ("cross-device") errno. The `std::fs::rename` fallback to
/// copy-then-remove only fires on this code.
#[cfg(unix)]
const EXDEV: i32 = libc::EXDEV;

/// Windows uses `ERROR_NOT_SAME_DEVICE` (17) as its cross-volume
/// signal; `std::fs::rename` surfaces it through `raw_os_error()` the
/// same way Unix surfaces `EXDEV`. We don't currently test on Windows
/// but wire the constant so the fallback is consistent.
#[cfg(windows)]
const EXDEV: i32 = 17;

#[cfg(not(any(unix, windows)))]
const EXDEV: i32 = 18;

/// Move `src` to `dst`. Uses `rename` first, then falls back to
/// copy-then-remove on `EXDEV` (cross-device) so archives on a
/// different mount from the working tree still work.
///
/// Dispatches on `src.is_dir()`: directories copy recursively, files
/// via a single `std::fs::copy`. The two old helpers
/// (`move_file_atomic`, `move_dir_atomic`) were identical modulo that
/// one branch — collapsing them keeps the cross-device semantics in a
/// single implementation shared by `specify_merge::slice` (archive
/// move) and `specify_change::plan` (plan archive move).
///
/// # Errors
///
/// Returns `Error::Io` on rename / copy / remove failures.
pub fn move_atomic(src: &Path, dst: &Path) -> Result<(), Error> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(EXDEV) => {
            if src.is_dir() {
                copy_dir_recursive(src, dst)?;
                std::fs::remove_dir_all(src)?;
            } else {
                std::fs::copy(src, dst)?;
                std::fs::remove_file(src)?;
            }
            Ok(())
        }
        Err(err) => Err(Error::Io(err)),
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            symlink(&link_target, &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    match std::fs::metadata(original) {
        Ok(meta) if meta.is_dir() => std::os::windows::fs::symlink_dir(original, link),
        _ => std::os::windows::fs::symlink_file(original, link),
    }
}

#[cfg(not(any(unix, windows)))]
fn symlink(_original: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "symlinks unsupported on this platform"))
}
