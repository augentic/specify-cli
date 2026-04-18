//! Verb-level operations on a Specify change directory.
//!
//! `actions` turns the static lifecycle state machine in the crate root
//! into transactional filesystem operations: creating a fresh change
//! directory, transitioning its `.metadata.yaml` status with the
//! associated timestamp write, scanning `specs/` for `touched_specs`,
//! detecting overlap against other active changes, and archiving
//! (`archive` / `drop`) into `.specify/archive/YYYY-MM-DD-<name>/`.
//!
//! Every verb is expressed as a free function rather than a struct method
//! so the CLI can dispatch each subcommand with one import per verb. They
//! all round-trip through [`ChangeMetadata::save`] for the metadata writes
//! and share the cross-device-safe [`move_dir_atomic`] helper for archive
//! moves.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use specify_error::Error;

use crate::{ChangeMetadata, LifecycleStatus, Outcome, Phase, PhaseOutcome, SpecType, TouchedSpec};

/// What to do when `Change::create` finds an existing directory at the
/// target path.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CreateIfExists {
    /// Default. Refuse and return `Error::Config`.
    Fail,
    /// Reuse the existing directory. The function reloads its
    /// `.metadata.yaml` and returns it without writing. Intended for the
    /// define skill's "continue in-flight change" flow.
    Continue,
    /// Delete and recreate. Intended for the define skill's "restart"
    /// flow. The caller is expected to have already archived anything it
    /// wants to keep — this branch is destructive.
    Restart,
}

/// Outcome of [`create`], surfacing whether a new directory was written
/// or an existing one was reused.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateOutcome {
    pub change_dir: PathBuf,
    pub metadata: ChangeMetadata,
    /// `true` when the call created a new directory; `false` when an
    /// existing directory was reused (`CreateIfExists::Continue`).
    pub created: bool,
    /// `true` when the call replaced an existing directory
    /// (`CreateIfExists::Restart`).
    pub restarted: bool,
}

/// A capability-level conflict between two active changes both touching
/// the same spec.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlap {
    pub capability: String,
    pub other_change: String,
    pub our_spec_type: SpecType,
    pub other_spec_type: SpecType,
}

/// Validate a kebab-case change name.
///
/// Names must be non-empty, contain only `[a-z0-9-]`, and may not start,
/// end, or contain consecutive hyphens. Identical contract to the
/// `specify-change` naming rules in `rfcs/rfc-1-cli.md`.
pub fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::Config("change name cannot be empty".to_string()));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(Error::Config(format!(
            "change name `{name}` cannot start or end with a hyphen"
        )));
    }
    if name.contains("--") {
        return Err(Error::Config(format!(
            "change name `{name}` cannot contain consecutive hyphens"
        )));
    }
    for c in name.chars() {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
            return Err(Error::Config(format!(
                "change name `{name}` must be kebab-case (lowercase ascii, digits, hyphens only)"
            )));
        }
    }
    Ok(())
}

/// Create `<changes_dir>/<name>/` and seed an initial `.metadata.yaml`.
///
/// - `changes_dir` is expected to be `<project>/.specify/changes/`.
/// - `now` is plumbed in so tests can pin `created_at` deterministically.
///
/// On success returns a [`CreateOutcome`] with the resolved directory and
/// loaded metadata. Behaviour when the directory already exists is
/// governed by `if_exists` — see [`CreateIfExists`].
pub fn create(
    changes_dir: &Path, name: &str, schema: &str, if_exists: CreateIfExists, now: DateTime<Utc>,
) -> Result<CreateOutcome, Error> {
    validate_name(name)?;
    let change_dir = changes_dir.join(name);
    let metadata_path = ChangeMetadata::path(&change_dir);

    if change_dir.exists() {
        match if_exists {
            CreateIfExists::Fail => {
                return Err(Error::Config(format!(
                    "change `{name}` already exists at {}",
                    change_dir.display()
                )));
            }
            CreateIfExists::Continue => {
                if !metadata_path.exists() {
                    return Err(Error::Config(format!(
                        "change dir {} exists but has no .metadata.yaml; refusing to reuse",
                        change_dir.display()
                    )));
                }
                let metadata = ChangeMetadata::load(&change_dir)?;
                return Ok(CreateOutcome {
                    change_dir,
                    metadata,
                    created: false,
                    restarted: false,
                });
            }
            CreateIfExists::Restart => {
                std::fs::remove_dir_all(&change_dir)?;
            }
        }
    }

    std::fs::create_dir_all(change_dir.join("specs"))?;
    let metadata = ChangeMetadata {
        schema: schema.to_string(),
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
    metadata.save(&change_dir)?;

    Ok(CreateOutcome {
        change_dir,
        metadata,
        created: true,
        restarted: matches!(if_exists, CreateIfExists::Restart),
    })
}

/// Transition a change to `target` status and write the matching timestamp.
///
/// The transition is validated by
/// [`LifecycleStatus::transition`](crate::LifecycleStatus::transition) —
/// illegal edges return `Error::Lifecycle` without touching disk. On
/// success the metadata's `status` is updated, the appropriate
/// `*_at` timestamp is filled in (idempotent: an existing non-`None`
/// timestamp is preserved), and `.metadata.yaml` is rewritten atomically.
///
/// Returns the updated `ChangeMetadata`.
pub fn transition(
    change_dir: &Path, target: LifecycleStatus, now: DateTime<Utc>,
) -> Result<ChangeMetadata, Error> {
    let mut metadata = ChangeMetadata::load(change_dir)?;
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
    metadata.save(change_dir)?;
    Ok(metadata)
}

/// Scan `<change_dir>/specs/*` and classify each capability as
/// `new` or `modified` against `<specs_dir>/<name>/spec.md`.
///
/// Returns entries sorted by capability name for stable output. The
/// scan is non-destructive — it does not mutate `.metadata.yaml`. The
/// caller typically follows up with [`write_touched_specs`].
pub fn scan_touched_specs(change_dir: &Path, specs_dir: &Path) -> Result<Vec<TouchedSpec>, Error> {
    let specs_root = change_dir.join("specs");
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
        let spec_type = if baseline.is_file() { SpecType::Modified } else { SpecType::New };
        entries.push(TouchedSpec { name, spec_type });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Overwrite `.metadata.yaml`'s `touched_specs` with `entries`.
///
/// Leaves every other field on the struct untouched, including `status`.
pub fn write_touched_specs(
    change_dir: &Path, entries: Vec<TouchedSpec>,
) -> Result<ChangeMetadata, Error> {
    let mut metadata = ChangeMetadata::load(change_dir)?;
    metadata.touched_specs = entries;
    metadata.save(change_dir)?;
    Ok(metadata)
}

/// Detect overlap between this change's `touched_specs` and every other
/// active change's. "Active" means a directory under `changes_dir` that
/// has a `.metadata.yaml` and is not `change_name` itself.
///
/// Merged and dropped changes still appear on disk until the archive
/// move completes, so we additionally filter by status: only
/// non-terminal statuses participate. Archive directories under
/// `changes_dir` (e.g. `<changes_dir>/archive/...`) are not scanned.
pub fn overlap(changes_dir: &Path, change_name: &str) -> Result<Vec<Overlap>, Error> {
    let self_dir = changes_dir.join(change_name);
    let self_meta = ChangeMetadata::load(&self_dir)?;
    if self_meta.touched_specs.is_empty() {
        return Ok(Vec::new());
    }

    let mut overlaps: Vec<Overlap> = Vec::new();
    let Ok(entries) = std::fs::read_dir(changes_dir) else {
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
        if other_name == change_name || other_name == "archive" {
            continue;
        }
        if !ChangeMetadata::path(&other_path).exists() {
            continue;
        }
        let other_meta = ChangeMetadata::load(&other_path)?;
        if other_meta.status.is_terminal() {
            continue;
        }
        for ours in &self_meta.touched_specs {
            for theirs in &other_meta.touched_specs {
                if ours.name == theirs.name {
                    overlaps.push(Overlap {
                        capability: ours.name.clone(),
                        other_change: other_name.to_string(),
                        our_spec_type: ours.spec_type,
                        other_spec_type: theirs.spec_type,
                    });
                }
            }
        }
    }
    overlaps.sort_by(|a, b| {
        a.capability.cmp(&b.capability).then_with(|| a.other_change.cmp(&b.other_change))
    });
    Ok(overlaps)
}

/// Move `change_dir` to `<archive_dir>/YYYY-MM-DD-<change-name>/`.
///
/// This is the sole implementation of the archive move semantics; both
/// `specify change archive` and the `specify merge` success path route
/// through it. Does **not** touch `.metadata.yaml` — the caller is
/// responsible for any status transition before or after.
pub fn archive(
    change_dir: &Path, archive_dir: &Path, today: DateTime<Utc>,
) -> Result<PathBuf, Error> {
    let change_name = change_dir.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        Error::Config(format!("change dir {} has no basename", change_dir.display()))
    })?;
    let date = today.format("%Y-%m-%d").to_string();
    let target = archive_dir.join(format!("{date}-{change_name}"));
    std::fs::create_dir_all(archive_dir)?;
    move_dir_atomic(change_dir, &target)?;
    Ok(target)
}

/// Stamp the outcome of a phase run on `<change_dir>/.metadata.yaml`.
///
/// Sole writer of [`ChangeMetadata::outcome`]. The whole metadata file
/// is rewritten atomically via [`ChangeMetadata::save`] so a concurrent
/// reader never sees a half-written file. A new stamp replaces any
/// previous one — history lives in `journal.yaml` (L2.B), not here.
///
/// `now` is plumbed in so tests can pin `at` deterministically; the CLI
/// passes `Utc::now()`.
///
/// Returns the updated [`ChangeMetadata`].
pub fn phase_outcome(
    change_dir: &Path, phase: Phase, outcome: Outcome, summary: &str, context: Option<&str>,
    now: DateTime<Utc>,
) -> Result<ChangeMetadata, Error> {
    let mut metadata = ChangeMetadata::load(change_dir)?;
    metadata.outcome = Some(PhaseOutcome {
        phase,
        outcome,
        at: now.to_rfc3339(),
        summary: summary.to_string(),
        context: context.map(str::to_string),
    });
    metadata.save(change_dir)?;
    Ok(metadata)
}

/// Transition a change to `Dropped`, record the optional reason, then
/// archive. Returns the final archive path.
///
/// Valid from any non-terminal lifecycle state. Callers use this for
/// both failure ("dropped because build broke") and deferral ("blocked
/// on a design question") — the plan layer above turns the reason into
/// `failure-reason` or `block-reason`; here it's just free text.
pub fn drop(
    change_dir: &Path, archive_dir: &Path, reason: Option<&str>, now: DateTime<Utc>,
) -> Result<(ChangeMetadata, PathBuf), Error> {
    let mut metadata = ChangeMetadata::load(change_dir)?;
    metadata.status = metadata.status.transition(LifecycleStatus::Dropped)?;
    metadata.dropped_at = Some(format_rfc3339(now));
    if let Some(text) = reason {
        metadata.drop_reason = Some(text.to_string());
    }
    metadata.save(change_dir)?;
    let target = archive(change_dir, archive_dir, now)?;
    Ok((metadata, target))
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn format_rfc3339(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Move `src` to `dst`. Uses `rename` first, then falls back to
/// copy-then-remove on `EXDEV` (cross-device) so archives on a
/// different mount from the working tree still work.
///
/// Extracted from `specify_merge::change` — the two callers share a
/// single implementation so the semantics stay in lockstep.
pub(crate) fn move_dir_atomic(src: &Path, dst: &Path) -> Result<(), Error> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc_exdev()) => {
            copy_dir_recursive(src, dst)?;
            std::fs::remove_dir_all(src)?;
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

fn libc_exdev() -> i32 {
    18
}
