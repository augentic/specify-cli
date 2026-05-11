//! Slot-problem detection: read-only diagnostics that mirror the
//! refusals enforced by `sync_projects` for individual workspace slots.

use std::path::{Path, PathBuf};

use super::git::git_output_ok;
use super::status::SlotKind;
use super::{registry_symlink_target, workspace_base, workspace_slot_path};
use crate::registry::registry::RegistryProject;

/// A registry/workspace mismatch that would cause `workspace sync` to refuse a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotProblem {
    /// Machine-readable reason for the mismatch.
    pub reason: SlotProblemReason,
    /// Materialisation kind expected from the registry URL.
    pub expected_kind: SlotKind,
    /// Materialisation kind currently observed on disk, when inspectable.
    pub observed_kind: Option<SlotKind>,
    /// Registry URL expected for this slot.
    pub expected_url: String,
    /// Observed git origin URL for remote-backed slots, when readable.
    pub observed_url: Option<String>,
    /// Canonical filesystem target expected for symlink-backed slots.
    pub expected_target: Option<PathBuf>,
    /// Canonical filesystem target currently observed for symlink-backed slots.
    pub observed_target: Option<PathBuf>,
    pub(super) message: String,
}

impl SlotProblem {
    /// Human-readable diagnostic matching the refusal text from `workspace sync`.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Stable reason code for [`SlotProblem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotProblemReason {
    /// Project name cannot map to exactly `.specify/workspace/<project>/`.
    SlotPathEscapesWorkspace,
    /// The registry's local/relative URL no longer resolves.
    LocalTargetUnresolved,
    /// A local/relative registry entry points at a non-symlink slot.
    LocalSlotIsNotSymlink,
    /// A local/relative registry entry points at a symlink with the wrong target.
    LocalSymlinkTargetMismatch,
    /// A local/relative registry entry points at a broken symlink.
    LocalSymlinkBroken,
    /// A remote registry entry points at a symlink slot.
    RemoteSlotIsSymlink,
    /// A remote registry entry points at a non-directory slot.
    RemoteSlotIsNotDirectory,
    /// A remote registry entry points at a directory without `.git/`.
    RemoteSlotIsNotGitClone,
    /// A remote-backed clone has no readable `origin` remote.
    RemoteOriginMissing,
    /// A remote-backed clone's `origin` differs from the registry URL.
    RemoteOriginMismatch,
    /// Slot metadata could not be read.
    SlotMetadataUnreadable,
}

/// Inspect one registry project slot for the mismatch cases enforced by sync.
///
/// Returns `None` for a missing slot and for a slot that already matches the
/// registry. The function is read-only; callers such as doctor/status can use it
/// to report the same wrong-remote and wrong-symlink facts that sync refuses.
#[must_use]
pub fn slot_problem(project_dir: &Path, project: &RegistryProject) -> Option<SlotProblem> {
    let base = workspace_base(project_dir);
    match workspace_slot_path(&base, &project.name) {
        Ok(dest) => slot_problem_at(project_dir, project, &dest),
        Err(err) => Some(SlotProblem {
            reason: SlotProblemReason::SlotPathEscapesWorkspace,
            expected_kind: expected_slot_kind(project),
            observed_kind: None,
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: None,
            observed_target: None,
            message: err.to_string(),
        }),
    }
}

pub(super) fn slot_problem_at(
    project_dir: &Path, project: &RegistryProject, dest: &Path,
) -> Option<SlotProblem> {
    if project.is_local() {
        inspect_local_slot(project_dir, project, dest)
    } else {
        inspect_remote_slot(project, dest)
    }
}

pub(super) fn expected_slot_kind(project: &RegistryProject) -> SlotKind {
    if project.is_local() { SlotKind::Symlink } else { SlotKind::GitClone }
}

fn inspect_local_slot(
    project_dir: &Path, project: &RegistryProject, dest: &Path,
) -> Option<SlotProblem> {
    let meta = match std::fs::symlink_metadata(dest) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            return Some(SlotProblem {
                reason: SlotProblemReason::SlotMetadataUnreadable,
                expected_kind: SlotKind::Symlink,
                observed_kind: None,
                expected_url: project.url.clone(),
                observed_url: None,
                expected_target: None,
                observed_target: None,
                message: format!("failed to inspect `{}`: {err}", dest.display()),
            });
        }
    };

    let target = match registry_symlink_target(project_dir, &project.url) {
        Ok(target) => target,
        Err(err) => {
            return Some(SlotProblem {
                reason: SlotProblemReason::LocalTargetUnresolved,
                expected_kind: SlotKind::Symlink,
                observed_kind: Some(observed_slot_kind(&meta, dest)),
                expected_url: project.url.clone(),
                observed_url: None,
                expected_target: None,
                observed_target: None,
                message: err.to_string(),
            });
        }
    };

    if !meta.file_type().is_symlink() {
        return Some(SlotProblem {
            reason: SlotProblemReason::LocalSlotIsNotSymlink,
            expected_kind: SlotKind::Symlink,
            observed_kind: Some(observed_slot_kind(&meta, dest)),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: Some(target),
            observed_target: None,
            message: format!(
                ".specify/workspace/{} already exists and is not a symlink; remove it before re-syncing",
                dest.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            ),
        });
    }

    match std::fs::canonicalize(dest) {
        Ok(resolved) if resolved == target => None,
        Ok(resolved) => Some(SlotProblem {
            reason: SlotProblemReason::LocalSymlinkTargetMismatch,
            expected_kind: SlotKind::Symlink,
            observed_kind: Some(SlotKind::Symlink),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: Some(target.clone()),
            observed_target: Some(resolved.clone()),
            message: format!(
                ".specify/workspace/{} already exists as a symlink to {}; expected {} from registry url `{}`",
                dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                resolved.display(),
                target.display(),
                project.url
            ),
        }),
        Err(err) => Some(SlotProblem {
            reason: SlotProblemReason::LocalSymlinkBroken,
            expected_kind: SlotKind::Symlink,
            observed_kind: Some(SlotKind::Symlink),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: Some(target.clone()),
            observed_target: None,
            message: format!(
                ".specify/workspace/{} already exists as a broken symlink; expected {} from registry url `{}` ({err})",
                dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                target.display(),
                project.url
            ),
        }),
    }
}

fn inspect_remote_slot(project: &RegistryProject, dest: &Path) -> Option<SlotProblem> {
    let meta = match std::fs::symlink_metadata(dest) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            return Some(SlotProblem {
                reason: SlotProblemReason::SlotMetadataUnreadable,
                expected_kind: SlotKind::GitClone,
                observed_kind: None,
                expected_url: project.url.clone(),
                observed_url: None,
                expected_target: None,
                observed_target: None,
                message: format!("failed to inspect `{}`: {err}", dest.display()),
            });
        }
    };

    if meta.file_type().is_symlink() {
        return Some(SlotProblem {
            reason: SlotProblemReason::RemoteSlotIsSymlink,
            expected_kind: SlotKind::GitClone,
            observed_kind: Some(SlotKind::Symlink),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: None,
            observed_target: std::fs::canonicalize(dest).ok(),
            message: format!(
                "`{}` is a symlink, but registry url `{}` is remote-backed; remove the slot before re-syncing",
                dest.display(),
                project.url
            ),
        });
    }

    if !meta.is_dir() {
        return Some(SlotProblem {
            reason: SlotProblemReason::RemoteSlotIsNotDirectory,
            expected_kind: SlotKind::GitClone,
            observed_kind: Some(SlotKind::Other),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: None,
            observed_target: None,
            message: format!(
                "`{}` exists but is not a directory; remove it before re-syncing",
                dest.display()
            ),
        });
    }

    if !dest.join(".git").exists() {
        return Some(SlotProblem {
            reason: SlotProblemReason::RemoteSlotIsNotGitClone,
            expected_kind: SlotKind::GitClone,
            observed_kind: Some(SlotKind::Other),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: None,
            observed_target: None,
            message: format!(
                "`{}` exists but is not a git clone (no `.git/`); remove it or pick another registry name",
                dest.display()
            ),
        });
    }

    match git_output_ok(dest, &["remote", "get-url", "origin"]) {
        Some(actual) if actual == project.url => None,
        Some(actual) => Some(SlotProblem {
            reason: SlotProblemReason::RemoteOriginMismatch,
            expected_kind: SlotKind::GitClone,
            observed_kind: Some(SlotKind::GitClone),
            expected_url: project.url.clone(),
            observed_url: Some(actual.clone()),
            expected_target: None,
            observed_target: None,
            message: format!(
                "`{}` origin remote is `{actual}`, but registry url is `{}`; \
                 remove the slot or update registry.yaml before re-syncing",
                dest.display(),
                project.url
            ),
        }),
        None => Some(SlotProblem {
            reason: SlotProblemReason::RemoteOriginMissing,
            expected_kind: SlotKind::GitClone,
            observed_kind: Some(SlotKind::GitClone),
            expected_url: project.url.clone(),
            observed_url: None,
            expected_target: None,
            observed_target: None,
            message: format!(
                "`{}` has no origin remote; expected registry url `{}`",
                dest.display(),
                project.url
            ),
        }),
    }
}

fn observed_slot_kind(meta: &std::fs::Metadata, slot: &Path) -> SlotKind {
    if meta.file_type().is_symlink() {
        SlotKind::Symlink
    } else if meta.is_dir() && slot.join(".git").exists() {
        SlotKind::GitClone
    } else {
        SlotKind::Other
    }
}
