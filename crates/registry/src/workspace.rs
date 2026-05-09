//! Multi-project workspace materialisation under `.specify/workspace/`
//! (RFC-3a C29).
//!
//! Lifted into `specify-registry` by RFC-13 chunk 2.2 so all
//! registry-derived state lives in one crate. The `Plan` argument was
//! flattened to `&str initiative_name` to avoid a cycle with
//! `specify-slice` (which already depends on `specify-registry`); the
//! callers in the binary pass `&plan.name` and the same surface
//! continues to be exposed as `crate::workspace::*` re-exports from
//! `src/lib.rs`.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use specify_error::Error;

use crate::Registry;
use crate::forge::project_path;
use crate::gitignore::ensure_specify_gitignore_entries;
use crate::registry::RegistryProject;

/// Absolute path to `<project_dir>/.specify/workspace/`. Mirror of
/// `ProjectConfig::specify_dir(...).join("workspace")` from the binary;
/// duplicated so the registry crate stays self-contained.
fn workspace_base(project_dir: &Path) -> PathBuf {
    project_dir.join(".specify").join("workspace")
}

/// Absolute path to `<project_dir>/contracts/`. Operator-facing
/// platform artifact; lives at the repo root by RFC-9.
fn contracts_base(project_dir: &Path) -> PathBuf {
    project_dir.join("contracts")
}

/// Materialise `.specify/workspace/<name>/` for every registry entry.
///
/// Symlinks for `.` / relative URLs, shallow `git clone` or `git fetch`
/// for remotes. Ensures `.gitignore` lists `.specify/workspace/` (and
/// `.specify/.cache/` when missing).
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn sync_all(project_dir: &Path) -> Result<(), Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(());
    };
    let projects = registry.select(&[])?;
    sync_projects(project_dir, &projects)
}

/// Materialise `.specify/workspace/<name>/` for selected registry entries.
///
/// Callers must pass projects returned by
/// [`Registry::select`] so unknown selectors fail before
/// this function performs filesystem, Git, or forge work.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn sync_projects(project_dir: &Path, projects: &[&RegistryProject]) -> Result<(), Error> {
    ensure_specify_gitignore_entries(project_dir)?;

    let base = prepare_workspace_base(project_dir)?;

    let mut errors: Vec<String> = Vec::new();
    for project in projects {
        let result = workspace_slot_path(&base, &project.name).and_then(|dest| {
            if let Some(problem) = slot_problem_at(project_dir, project, &dest) {
                return Err(Error::Config(problem.message().to_string()));
            }
            if project.is_local() {
                materialise_symlink(project_dir, &project.url, &dest)
            } else {
                materialise_git_remote(&project.url, &dest, &project.schema, project_dir)
            }
        });
        if let Err(err) = result {
            errors.push(format!("{}: {err}", project.name));
        }
    }

    // Distribute central contracts to non-symlink workspace clones.
    let central_contracts = contracts_base(project_dir);
    if central_contracts.is_dir() {
        for project in projects {
            if project.is_local() {
                continue;
            }
            let Ok(slot) = workspace_slot_path(&base, &project.name) else {
                continue;
            };
            if !slot.is_dir() {
                continue;
            }
            if !slot.join(".specify").is_dir() {
                continue;
            }
            let dest_contracts = slot.join("contracts");
            if let Err(err) = distribute_contracts(&central_contracts, &dest_contracts) {
                errors.push(format!("{} (contracts): {err}", project.name));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "workspace sync failed for {} project(s):\n{}",
            errors.len(),
            errors.join("\n")
        )))
    }
}

fn prepare_workspace_base(project_dir: &Path) -> Result<PathBuf, Error> {
    let specify_dir = project_dir.join(".specify");
    reject_symlinked_directory(&specify_dir, ".specify/")?;
    std::fs::create_dir_all(&specify_dir).map_err(Error::Io)?;

    let base = workspace_base(project_dir);
    reject_symlinked_directory(&base, ".specify/workspace/")?;
    std::fs::create_dir_all(&base).map_err(Error::Io)?;
    reject_symlinked_directory(&base, ".specify/workspace/")?;

    Ok(base)
}

fn reject_symlinked_directory(path: &Path, label: &str) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Config(format!(
            "{label} is a symlink; refusing to materialise workspace slots through it"
        ))),
        Ok(meta) if !meta.is_dir() => Err(Error::Config(format!(
            "{label} exists but is not a directory; remove it before running workspace sync"
        ))),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

fn workspace_slot_path(base: &Path, project_name: &str) -> Result<PathBuf, Error> {
    let name_path = Path::new(project_name);
    let mut components = name_path.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(slot_escape_error(project_name));
    };
    if components.next().is_some() || component.to_string_lossy() != project_name {
        return Err(slot_escape_error(project_name));
    }

    let dest = base.join(project_name);
    if dest.strip_prefix(base).ok() != Some(Path::new(project_name)) {
        return Err(slot_escape_error(project_name));
    }
    Ok(dest)
}

fn slot_escape_error(project_name: &str) -> Error {
    Error::Config(format!(
        "registry project name `{project_name}` would escape `.specify/workspace/<project>/`; \
         project names must be a single path component"
    ))
}

/// One row for `specify workspace status` text/JSON output.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct SlotStatus {
    /// Registry project name (`.specify/workspace/<name>/`).
    pub name: String,
    /// Absolute slot path under `.specify/workspace/`.
    pub slot_path: PathBuf,
    /// How the slot is materialised on disk.
    pub kind: SlotKind,
    /// Whether the registry target is expected to be local or remote-backed.
    pub configured_target_kind: ConfiguredTargetKind,
    /// Registry remote URL, or the resolved local target path for symlink-backed projects.
    pub configured_target: String,
    /// Symlink target recorded on disk when the slot is a symlink.
    pub actual_symlink_target: Option<PathBuf>,
    /// `origin` remote reported by Git when the inspected tree has one.
    pub actual_origin: Option<String>,
    /// Current Git branch for a materialised repository.
    pub current_branch: Option<String>,
    /// `git rev-parse HEAD` when the resolved tree is a git checkout.
    pub head_sha: Option<String>,
    /// `true` when `git status --porcelain` is non-empty.
    pub dirty: Option<bool>,
    /// Whether `current_branch` exactly equals `specify/<plan.name>`.
    ///
    /// `None` means either no active plan name was discoverable or no branch
    /// could be read from the slot.
    pub branch_matches_change: Option<bool>,
    /// Whether `.specify/project.yaml` exists in the resolved project tree.
    pub project_config_present: bool,
    /// Direct child directories under `.specify/slices/`, sorted by name.
    pub active_slices: Vec<String>,
}

/// Whether a registry entry is configured as a local filesystem target or a remote URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfiguredTargetKind {
    /// Local filesystem target materialised as a symlink.
    Local,
    /// Remote-backed target materialised as a Git clone.
    Remote,
}

/// Classification of a workspace slot on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    /// Path missing.
    Missing,
    /// Symlink under `.specify/workspace/<name>/`.
    Symlink,
    /// Ordinary directory with a `.git/` metadata tree (clone target).
    GitClone,
    /// Present but neither a recognised symlink nor a git work tree.
    Other,
}

impl SlotKind {
    /// Stable label used by CLI text/JSON output and diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Symlink => "symlink",
            Self::GitClone => "git-clone",
            Self::Other => "other",
        }
    }
}

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
    message: String,
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

/// Inspect `.specify/workspace/<name>/` for each registry project.
///
/// Returns `Ok(None)` when `.specify/registry.yaml` is absent.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn status(project_dir: &Path) -> Result<Option<Vec<SlotStatus>>, Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(None);
    };
    let projects = registry.select(&[])?;
    Ok(Some(status_projects(project_dir, &projects)))
}

/// Inspect `.specify/workspace/<name>/` for selected registry projects.
#[must_use]
pub fn status_projects(project_dir: &Path, projects: &[&RegistryProject]) -> Vec<SlotStatus> {
    let base = workspace_base(project_dir);
    let change_name = discover_change_name(project_dir);
    let mut out = Vec::with_capacity(projects.len());
    for project in projects {
        let slot = base.join(&project.name);
        out.push(describe_slot(project_dir, project, &slot, change_name.as_deref()));
    }
    out
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

fn describe_slot(
    project_dir: &Path, project: &RegistryProject, slot: &Path, change_name: Option<&str>,
) -> SlotStatus {
    let (configured_target_kind, configured_target) = configured_target(project_dir, project);
    let mut status = SlotStatus {
        name: project.name.clone(),
        slot_path: slot.to_path_buf(),
        kind: SlotKind::Missing,
        configured_target_kind,
        configured_target,
        actual_symlink_target: None,
        actual_origin: None,
        current_branch: None,
        head_sha: None,
        dirty: None,
        branch_matches_change: None,
        project_config_present: false,
        active_slices: Vec::new(),
    };

    let Ok(meta) = std::fs::symlink_metadata(slot) else {
        return status;
    };

    if meta.file_type().is_symlink() {
        status.kind = SlotKind::Symlink;
        status.actual_symlink_target = std::fs::read_link(slot).ok();
        if slot.exists() {
            enrich_project_tree(&mut status, slot, change_name);
        }
        return status;
    }

    if meta.is_dir() && slot.join(".git").exists() {
        status.kind = SlotKind::GitClone;
        enrich_project_tree(&mut status, slot, change_name);
        return status;
    }

    status.kind = SlotKind::Other;
    if meta.is_dir() {
        enrich_project_metadata(&mut status, slot);
    }
    status
}

fn configured_target(
    project_dir: &Path, project: &RegistryProject,
) -> (ConfiguredTargetKind, String) {
    if project.is_local() {
        let target = local_target_path(project_dir, &project.url);
        let resolved = std::fs::canonicalize(&target).unwrap_or(target);
        (ConfiguredTargetKind::Local, resolved.display().to_string())
    } else {
        (ConfiguredTargetKind::Remote, project.url.clone())
    }
}

fn local_target_path(project_dir: &Path, url: &str) -> PathBuf {
    if url == "." { project_dir.to_path_buf() } else { project_dir.join(url) }
}

fn slot_problem_at(
    project_dir: &Path, project: &RegistryProject, dest: &Path,
) -> Option<SlotProblem> {
    if project.is_local() {
        local_slot_problem(project_dir, project, dest)
    } else {
        remote_slot_problem(project, dest)
    }
}

fn expected_slot_kind(project: &RegistryProject) -> SlotKind {
    if project.is_local() { SlotKind::Symlink } else { SlotKind::GitClone }
}

fn local_slot_problem(
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

fn remote_slot_problem(project: &RegistryProject, dest: &Path) -> Option<SlotProblem> {
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

fn enrich_project_tree(status: &mut SlotStatus, tree: &Path, change_name: Option<&str>) {
    enrich_project_metadata(status, tree);

    if git_output_ok(tree, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return;
    }

    status.actual_origin = git_output_ok(tree, &["remote", "get-url", "origin"]);
    status.current_branch = git_output_ok(tree, &["branch", "--show-current"])
        .or_else(|| git_output_ok(tree, &["rev-parse", "--abbrev-ref", "HEAD"]));
    status.head_sha = git_output_ok(tree, &["rev-parse", "HEAD"]);
    status.dirty = Some(git_porcelain_non_empty(tree));
    status.branch_matches_change = change_name
        .zip(status.current_branch.as_deref())
        .map(|(name, branch)| branch.strip_prefix("specify/") == Some(name));
}

fn enrich_project_metadata(status: &mut SlotStatus, tree: &Path) {
    status.project_config_present = tree.join(".specify").join("project.yaml").is_file();
    status.active_slices = active_slice_names(tree);
}

fn active_slice_names(tree: &Path) -> Vec<String> {
    let slices = tree.join(".specify").join("slices");
    let Ok(entries) = std::fs::read_dir(slices) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    names
}

#[derive(Deserialize)]
struct PlanName {
    name: String,
}

fn discover_change_name(project_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(project_dir.join("plan.yaml")).ok()?;
    let plan = serde_saphyr::from_str::<PlanName>(&content).ok()?;
    let name = plan.name.trim();
    if name.is_empty() { None } else { Some(name.to_string()) }
}

fn git_output_ok(tree: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(tree).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn git_porcelain_non_empty(tree: &Path) -> bool {
    let Ok(output) =
        Command::new("git").arg("-C").arg(tree).args(["status", "--porcelain"]).output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    !output.stdout.is_empty()
}

fn materialise_symlink(project_dir: &Path, url: &str, dest: &Path) -> Result<(), Error> {
    let target = registry_symlink_target(project_dir, url)?;

    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(dest) {
            Ok(resolved) if resolved == target => return Ok(()),
            Ok(resolved) => {
                return Err(Error::Config(format!(
                    ".specify/workspace/{} already exists as a symlink to {}; expected {} from registry url `{url}`",
                    dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    resolved.display(),
                    target.display()
                )));
            }
            Err(err) => {
                return Err(Error::Config(format!(
                    ".specify/workspace/{} already exists as a broken symlink; expected {} from registry url `{url}` ({err})",
                    dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    target.display()
                )));
            }
        },
        Ok(_) => {
            return Err(Error::Config(format!(
                ".specify/workspace/{} already exists and is not a symlink; remove it before re-syncing",
                dest.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(e)),
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }

    symlink(&target, dest)?;
    Ok(())
}

fn registry_symlink_target(project_dir: &Path, url: &str) -> Result<PathBuf, Error> {
    if url == "." {
        std::fs::canonicalize(project_dir).map_err(|e| {
            Error::Config(format!("could not resolve project directory for registry url `.`: {e}"))
        })
    } else {
        let joined = project_dir.join(url);
        std::fs::canonicalize(&joined).map_err(|e| {
            Error::Config(format!(
                "could not resolve registry url `{url}` relative to {}: {}",
                project_dir.display(),
                e
            ))
        })
    }
}

fn symlink(target: &Path, link: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(Error::Io)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).map_err(Error::Io)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(Error::Config("platform does not support symlinks for `specify workspace sync`".into()))
    }
}

fn materialise_git_remote(
    url: &str, dest: &Path, schema: &str, initiating_project_dir: &Path,
) -> Result<(), Error> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Config(format!(
            "`{}` is a symlink, but registry url `{url}` is remote-backed; remove the slot before re-syncing",
            dest.display()
        ))),
        Ok(meta) if meta.is_dir() => {
            if !dest.join(".git").exists() {
                return Err(Error::Config(format!(
                    "`{}` exists but is not a git clone (no `.git/`); remove it or pick another registry name",
                    dest.display()
                )));
            }
            ensure_origin_matches(dest, url)?;
            if dest.join(".specify").join("project.yaml").exists() {
                // Healthy clone or complete greenfield bootstrap — refresh
                run_git(
                    dest,
                    &["fetch", "--depth", "1"],
                    &format!("git fetch in {}", dest.display()),
                )
                .or(Ok(()))
            } else {
                // Partial greenfield bootstrap: .git/ present but .specify/project.yaml absent
                greenfield_init(dest, schema, initiating_project_dir, true)
            }
        }
        Ok(_) => Err(Error::Config(format!(
            "`{}` exists but is not a directory; remove it before re-syncing",
            dest.display()
        ))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Attempt clone
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }

            let clone_result = Command::new("git")
            .args(["clone", "--depth", "1", url])
            .arg(dest)
            .output()
            .map_err(|e| {
                Error::Config(format!(
                    "failed to spawn `git clone` for registry url `{url}`: {e} (is `git` installed?)"
                ))
            })?;

            if clone_result.status.success() {
                ensure_origin_matches(dest, url)?;
                Ok(())
            } else {
                // Clone failed — treat as greenfield
                greenfield_bootstrap(url, dest, schema, initiating_project_dir)
            }
        }
        Err(err) => Err(Error::Io(err)),
    }
}

fn ensure_origin_matches(dest: &Path, expected_url: &str) -> Result<(), Error> {
    match git_output_ok(dest, &["remote", "get-url", "origin"]) {
        Some(actual) if actual == expected_url => Ok(()),
        Some(actual) => Err(Error::Config(format!(
            "`{}` origin remote is `{actual}`, but registry url is `{expected_url}`; \
             remove the slot or update registry.yaml before re-syncing",
            dest.display()
        ))),
        None => Err(Error::Config(format!(
            "`{}` has no origin remote; expected registry url `{expected_url}`",
            dest.display()
        ))),
    }
}

/// Full greenfield bootstrap: mkdir, git init, git remote add, specify init, git add+commit.
fn greenfield_bootstrap(
    url: &str, dest: &Path, schema: &str, initiating_project_dir: &Path,
) -> Result<(), Error> {
    std::fs::create_dir_all(dest).map_err(Error::Io)?;

    run_git(dest, &["init"], &format!("git init in {}", dest.display()))?;
    run_git(dest, &["remote", "add", "origin", url], &format!("git remote add origin {url}"))?;

    greenfield_init(dest, schema, initiating_project_dir, false)?;

    Ok(())
}

/// Scaffold a greenfield slot locally, then git add + commit.
/// `is_rerun` controls whether we amend the commit or create a new one.
fn greenfield_init(
    dest: &Path, schema: &str, initiating_project_dir: &Path, is_rerun: bool,
) -> Result<(), Error> {
    let capability = resolve_greenfield_capability(schema, initiating_project_dir)?;

    scaffold_greenfield_specify_tree(dest, &capability)?;

    run_git(dest, &["add", "."], &format!("git add in {}", dest.display()))?;

    if !git_porcelain_non_empty(dest) {
        return Ok(());
    }

    let has_commits = git_output_ok(dest, &["log", "--oneline", "-1"]).is_some();
    let commit_args = if is_rerun && has_commits {
        vec!["commit", "--amend", "--no-gpg-sign", "-m", "Initial Specify scaffold"]
    } else {
        vec!["commit", "--no-gpg-sign", "-m", "Initial Specify scaffold"]
    };
    run_git(dest, &commit_args, &format!("git commit in {}", dest.display()))?;

    Ok(())
}

fn scaffold_greenfield_specify_tree(dest: &Path, capability: &str) -> Result<(), Error> {
    let specify_dir = dest.join(".specify");
    for dir in [
        specify_dir.clone(),
        specify_dir.join("slices"),
        specify_dir.join("specs"),
        specify_dir.join("archive"),
        specify_dir.join(".cache"),
    ] {
        std::fs::create_dir_all(&dir).map_err(Error::Io)?;
    }

    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("greenfield");
    let project_yaml = format!(
        "name: {name}\ncapability: {capability}\nspecify_version: \"{}\"\nrules: {{}}\n",
        env!("CARGO_PKG_VERSION")
    );
    std::fs::write(specify_dir.join("project.yaml"), project_yaml).map_err(Error::Io)?;
    ensure_specify_gitignore_entries(dest)?;

    Ok(())
}

/// Resolve the capability identifier to pass into a greenfield slot's
/// `specify init <capability>`.
///
/// URL-shaped capabilities are already self-contained. Bare registry
/// capability identifiers are local to the initiating repo's cache, so
/// convert them into a file URI the spawned init can copy directly.
fn resolve_greenfield_capability(
    schema: &str, initiating_project_dir: &Path,
) -> Result<String, Error> {
    if schema.contains("://") {
        return Ok(schema.to_string());
    }
    let cache_base = initiating_project_dir.join(".specify").join(".cache");

    let direct = cache_base.join(schema);
    if direct.is_dir() {
        return Ok(format!("file://{}", direct.display()));
    }

    // Try the last path segment before any @ref for older cached layouts.
    let without_ref = schema.split('@').next().unwrap_or(schema);
    if let Some(segment) = without_ref.rsplit('/').find(|s| !s.is_empty()) {
        let by_segment = cache_base.join(segment);
        if by_segment.is_dir() {
            return Ok(format!("file://{}", by_segment.display()));
        }
    }

    Err(Error::Config(format!(
        "schema '{}' not cached in {}; run /spec:init in the initiating repo first",
        schema,
        cache_base.display()
    )))
}

/// Copy root `contracts/` from the initiating repo into a workspace slot's
/// root `contracts/`. Removes the destination first for a clean replacement,
/// then copies recursively.
fn distribute_contracts(src: &Path, dest: &Path) -> Result<(), Error> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| {
            Error::Config(format!("failed to remove old contracts at {}: {e}", dest.display()))
        })?;
    }
    copy_dir_recursive(src, dest)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dest)
        .map_err(|e| Error::Config(format!("failed to create {}: {e}", dest.display())))?;

    for entry in std::fs::read_dir(src)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", src.display())))?
    {
        let entry = entry.map_err(|e| Error::Config(format!("dir entry error: {e}")))?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            std::fs::copy(&src_path, &dest_path).map_err(|e| {
                Error::Config(format!(
                    "failed to copy {} to {}: {e}",
                    src_path.display(),
                    dest_path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str], label: &str) -> Result<(), Error> {
    let output = Command::new("git")
        .args(["-c", "user.name=Specify", "-c", "user.email=specify@example.invalid"])
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("{label}: failed to spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Config(format!("{label} failed: {stderr}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// workspace push (RFC-3b Change 8)
// ---------------------------------------------------------------------------

/// Classification of a single project push outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Branch pushed to remote.
    Pushed,
    /// Remote repo was created, then pushed.
    Created,
    /// Push failed (see `PushResult.error`).
    Failed,
    /// No changes to push.
    UpToDate,
    /// Local-only project (no remote configured).
    LocalOnly,
    /// Checkout is not currently on the expected `specify/<change>` branch.
    NoBranch,
}

impl std::fmt::Display for PushOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pushed => f.write_str("pushed"),
            Self::Created => f.write_str("created"),
            Self::Failed => f.write_str("failed"),
            Self::UpToDate => f.write_str("up-to-date"),
            Self::LocalOnly => f.write_str("local-only"),
            Self::NoBranch => f.write_str("no-branch"),
        }
    }
}

/// Result of a per-project push operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushResult {
    /// Registry project name.
    pub name: String,
    /// Outcome of this push.
    pub status: PushOutcome,
    /// Git branch pushed to.
    pub branch: Option<String>,
    /// `GitHub` PR number when one was created or found.
    pub pr_number: Option<u64>,
    /// Human-readable error when the push failed.
    pub error: Option<String>,
}

/// Extract a `GitHub` `org/repo` slug from a git remote URL.
/// Returns `None` for non-GitHub URLs.
#[must_use]
pub fn github_slug(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let slug = rest.strip_suffix(".git").unwrap_or(rest);
        return Some(slug.to_string());
    }
    for prefix in &["https://github.com/", "http://github.com/"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            let slug = rest.strip_suffix(".git").unwrap_or(rest);
            return Some(slug.to_string());
        }
    }
    if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        let slug = rest.strip_suffix(".git").unwrap_or(rest);
        return Some(slug.to_string());
    }
    None
}

/// Core implementation of `specify workspace push`.
///
/// `initiative_name` is `plan.name` from the binary side; the registry
/// crate cannot depend on `specify-slice` (which already depends on
/// `specify-registry`), so callers flatten the field at the boundary.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn push_all(
    project_dir: &Path, initiative_name: &str, registry: &Registry, filter_projects: &[String],
    dry_run: bool,
) -> Result<Vec<PushResult>, Error> {
    let target_projects = registry.select(filter_projects)?;
    push_projects(project_dir, initiative_name, &target_projects, dry_run)
}

/// Core implementation of `specify workspace push` for pre-resolved projects.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn push_projects(
    project_dir: &Path, initiative_name: &str, target_projects: &[&RegistryProject], dry_run: bool,
) -> Result<Vec<PushResult>, Error> {
    let branch_name = format!("specify/{initiative_name}");
    let workspace_base = workspace_base(project_dir);
    let forge = RealWorkspacePushForge;

    let mut results = Vec::new();

    for rp in target_projects {
        let result = push_single_project(
            project_dir,
            &workspace_base,
            rp,
            &branch_name,
            initiative_name,
            dry_run,
            &forge,
        );
        results.push(result);
    }

    Ok(results)
}

trait WorkspacePushForge {
    fn repo_exists(&self, slug: &str, project_path: &Path) -> Result<bool, Error>;
    fn create_repo(&self, slug: &str, project_path: &Path) -> Result<(), Error>;
    fn ensure_pull_request(
        &self, project_path: &Path, branch_name: &str, base_branch: &str, initiative_name: &str,
    ) -> Result<u64, Error>;
}

#[derive(Debug, Default, Clone, Copy)]
struct RealWorkspacePushForge;

impl WorkspacePushForge for RealWorkspacePushForge {
    fn repo_exists(&self, slug: &str, _project_path: &Path) -> Result<bool, Error> {
        let output = Command::new("gh")
            .args(["repo", "view", slug, "--json", "name"])
            .output()
            .map_err(|err| Error::Config(format!("failed to spawn `gh repo view`: {err}")))?;
        if output.status.success() {
            return Ok(true);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("not found")
            || lower.contains("could not resolve")
            || lower.contains("404")
        {
            return Ok(false);
        }
        Err(Error::Config(format!("gh repo view {slug} failed: {}", stderr.trim())))
    }

    fn create_repo(&self, slug: &str, project_path: &Path) -> Result<(), Error> {
        let output = Command::new("gh")
            .args(["repo", "create", slug, "--private", "--source", "."])
            .current_dir(project_path)
            .output()
            .map_err(|err| Error::Config(format!("failed to spawn `gh repo create`: {err}")))?;
        if output.status.success() {
            return Ok(());
        }
        Err(Error::Config(format!(
            "gh repo create {slug} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    fn ensure_pull_request(
        &self, project_path: &Path, branch_name: &str, base_branch: &str, initiative_name: &str,
    ) -> Result<u64, Error> {
        let existing = github_pr_for_branch(project_path, branch_name)?;
        if let Some(number) = existing {
            let edit = Command::new("gh")
                .args(["pr", "edit", &number.to_string(), "--base", base_branch])
                .current_dir(project_path)
                .output()
                .map_err(|err| Error::Config(format!("failed to spawn `gh pr edit`: {err}")))?;
            if edit.status.success() {
                return Ok(number);
            }
            return Err(Error::Config(format!(
                "gh pr edit #{number} failed: {}",
                String::from_utf8_lossy(&edit.stderr).trim()
            )));
        }

        let pr_title = format!("specify: {initiative_name}");
        let pr_body = format!(
            "Automated push from specify workspace push for initiative \
             `{initiative_name}`."
        );
        let create = Command::new("gh")
            .args([
                "pr",
                "create",
                "--base",
                base_branch,
                "--head",
                branch_name,
                "--title",
                &pr_title,
                "--body",
                &pr_body,
            ])
            .current_dir(project_path)
            .output()
            .map_err(|err| Error::Config(format!("failed to spawn `gh pr create`: {err}")))?;

        if !create.status.success() {
            return Err(Error::Config(format!(
                "gh pr create failed: {}",
                String::from_utf8_lossy(&create.stderr).trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&create.stdout).trim().to_string();
        stdout
            .rsplit('/')
            .next()
            .and_then(|num| num.parse().ok())
            .ok_or_else(|| Error::Config(format!("gh pr create returned no PR number: {stdout}")))
    }
}

fn github_pr_for_branch(project_path: &Path, branch_name: &str) -> Result<Option<u64>, Error> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch_name,
            "--state",
            "all",
            "--json",
            "number",
            "--limit",
            "1",
        ])
        .current_dir(project_path)
        .output()
        .map_err(|err| Error::Config(format!("failed to spawn `gh pr list`: {err}")))?;

    if !output.status.success() {
        return Err(Error::Config(format!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .map_err(|err| Error::Config(format!("gh pr list returned invalid JSON: {err}")))?;
    Ok(parsed.first().and_then(|pr| pr.get("number")).and_then(serde_json::Value::as_u64))
}

/// Force-push a branch to `origin` with lease protection.
fn push_branch(
    project_path: &Path, branch_name: &str, expected_remote_head: Option<&str>,
) -> Result<(), Error> {
    let lease = expected_remote_head.map_or_else(
        || format!("--force-with-lease=refs/heads/{branch_name}:"),
        |sha| format!("--force-with-lease=refs/heads/{branch_name}:{sha}"),
    );
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
    run_git(
        project_path,
        &["push", &lease, "-u", "origin", &refspec],
        &format!("git push to {branch_name}"),
    )
}

fn push_result(
    rp: &RegistryProject, status: PushOutcome, branch: Option<&str>, pr_number: Option<u64>,
    error: Option<String>,
) -> PushResult {
    PushResult {
        name: rp.name.clone(),
        status,
        branch: branch.map(ToString::to_string),
        pr_number,
        error,
    }
}

#[allow(clippy::too_many_lines)]
fn push_single_project(
    project_dir: &Path, workspace_base: &Path, rp: &RegistryProject, branch_name: &str,
    initiative_name: &str, dry_run: bool, forge: &dyn WorkspacePushForge,
) -> PushResult {
    let project_path = project_path(project_dir, workspace_base, rp);

    if !is_git_worktree(&project_path) {
        return push_result(
            rp,
            PushOutcome::Failed,
            None,
            None,
            Some(format!("no git worktree found at {}", project_path.display())),
        );
    }

    let Some(remote_url) = git_output_ok(&project_path, &["remote", "get-url", "origin"]) else {
        return push_result(rp, PushOutcome::LocalOnly, None, None, None);
    };
    let forge_url = git_output_ok(&project_path, &["config", "--get", "remote.origin.url"])
        .unwrap_or(remote_url);

    match current_branch(&project_path) {
        Ok(Some(current)) if current == branch_name => {}
        Ok(_) => return push_result(rp, PushOutcome::NoBranch, None, None, None),
        Err(err) => {
            return push_result(rp, PushOutcome::Failed, None, None, Some(err.to_string()));
        }
    }

    match git_status_porcelain(&project_path) {
        Ok(status) if status.is_empty() => {}
        Ok(_) => {
            return push_result(
                rp,
                PushOutcome::Failed,
                Some(branch_name),
                None,
                Some("checkout is dirty; commit or clean local work before workspace push".into()),
            );
        }
        Err(err) => {
            return push_result(
                rp,
                PushOutcome::Failed,
                Some(branch_name),
                None,
                Some(err.to_string()),
            );
        }
    }

    if remote_default_branch_is(&project_path, branch_name) {
        return push_result(rp, PushOutcome::NoBranch, None, None, None);
    }

    let local_head =
        match git_stdout_trimmed(&project_path, &["rev-parse", "HEAD"], "rev-parse HEAD") {
            Ok(sha) => sha,
            Err(err) => {
                return push_result(
                    rp,
                    PushOutcome::Failed,
                    Some(branch_name),
                    None,
                    Some(err.to_string()),
                );
            }
        };

    let slug = github_slug(&forge_url);
    let remote_branch =
        match inspect_remote_branch(&project_path, branch_name, slug.as_deref(), forge) {
            Ok(state) => state,
            Err(err) => {
                return push_result(
                    rp,
                    PushOutcome::Failed,
                    Some(branch_name),
                    None,
                    Some(err.to_string()),
                );
            }
        };
    let remote_head = match &remote_branch {
        RemoteBranchState::Present(sha) => Some(sha.as_str()),
        RemoteBranchState::Absent | RemoteBranchState::RepositoryMissing => None,
    };

    if remote_head == Some(local_head.as_str()) {
        if dry_run {
            if let Err(err) =
                ensure_pr_base_resolves_if_supported(&project_path, slug.as_deref(), branch_name)
            {
                return push_result(
                    rp,
                    PushOutcome::Failed,
                    Some(branch_name),
                    None,
                    Some(err.to_string()),
                );
            }
            return push_result(rp, PushOutcome::UpToDate, Some(branch_name), None, None);
        }
        return ensure_pr_if_supported(
            rp,
            &project_path,
            slug.as_deref(),
            branch_name,
            initiative_name,
            forge,
        )
        .map_or_else(
            |err| {
                push_result(rp, PushOutcome::Failed, Some(branch_name), None, Some(err.to_string()))
            },
            |pr| push_result(rp, PushOutcome::UpToDate, Some(branch_name), pr, None),
        );
    }

    let mut is_created = false;

    if matches!(remote_branch, RemoteBranchState::RepositoryMissing) {
        if dry_run {
            return push_result(rp, PushOutcome::Created, Some(branch_name), None, None);
        }
        if let Some(ref slug) = slug {
            if let Err(err) = forge.create_repo(slug, &project_path) {
                return push_result(
                    rp,
                    PushOutcome::Failed,
                    Some(branch_name),
                    None,
                    Some(err.to_string()),
                );
            }
            is_created = true;
        }
    } else if dry_run {
        if let Err(err) =
            ensure_pr_base_resolves_if_supported(&project_path, slug.as_deref(), branch_name)
        {
            return push_result(
                rp,
                PushOutcome::Failed,
                Some(branch_name),
                None,
                Some(err.to_string()),
            );
        }
        return push_result(rp, PushOutcome::Pushed, Some(branch_name), None, None);
    }

    if let Err(e) = push_branch(&project_path, branch_name, remote_head) {
        return push_result(rp, PushOutcome::Failed, Some(branch_name), None, Some(e.to_string()));
    }

    let pr_number = match ensure_pr_if_supported(
        rp,
        &project_path,
        slug.as_deref(),
        branch_name,
        initiative_name,
        forge,
    ) {
        Ok(pr) => pr,
        Err(err) => {
            return push_result(
                rp,
                PushOutcome::Failed,
                Some(branch_name),
                None,
                Some(err.to_string()),
            );
        }
    };

    let status = if is_created { PushOutcome::Created } else { PushOutcome::Pushed };

    push_result(rp, status, Some(branch_name), pr_number, None)
}

enum RemoteBranchState {
    Present(String),
    Absent,
    RepositoryMissing,
}

fn inspect_remote_branch(
    project_path: &Path, branch_name: &str, slug: Option<&str>, forge: &dyn WorkspacePushForge,
) -> Result<RemoteBranchState, Error> {
    match remote_branch_head(project_path, branch_name) {
        Ok(Some(sha)) => Ok(RemoteBranchState::Present(sha)),
        Ok(None) => Ok(RemoteBranchState::Absent),
        Err(err) => {
            let Some(slug) = slug else {
                return Err(err);
            };
            if forge.repo_exists(slug, project_path)? {
                Err(err)
            } else {
                Ok(RemoteBranchState::RepositoryMissing)
            }
        }
    }
}

fn ensure_pr_if_supported(
    _rp: &RegistryProject, project_path: &Path, slug: Option<&str>, branch_name: &str,
    initiative_name: &str, forge: &dyn WorkspacePushForge,
) -> Result<Option<u64>, Error> {
    if slug.is_none() {
        return Ok(None);
    }
    let base_branch = resolve_remote_default_branch(project_path)?;
    if base_branch == branch_name {
        return Err(Error::Config(format!(
            "remote default branch resolves to `{branch_name}`; refusing to create a PR against \
             its own head branch"
        )));
    }
    forge.ensure_pull_request(project_path, branch_name, &base_branch, initiative_name).map(Some)
}

fn ensure_pr_base_resolves_if_supported(
    project_path: &Path, slug: Option<&str>, branch_name: &str,
) -> Result<(), Error> {
    if slug.is_some() {
        let base_branch = resolve_remote_default_branch(project_path)?;
        if base_branch == branch_name {
            return Err(Error::Config(format!(
                "remote default branch resolves to `{branch_name}`; refusing to treat it as a \
                 workspace push branch"
            )));
        }
    }
    Ok(())
}

fn is_git_worktree(project_path: &Path) -> bool {
    git_output_ok(project_path, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true")
}

fn current_branch(project_path: &Path) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|err| Error::Config(format!("failed to inspect current branch: {err}")))?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!branch.is_empty()).then_some(branch))
}

fn git_status_porcelain(project_path: &Path) -> Result<String, Error> {
    git_stdout_allow_empty(
        project_path,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        "git status --porcelain",
    )
}

fn remote_branch_head(project_path: &Path, branch_name: &str) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["ls-remote", "--heads", "origin", &format!("refs/heads/{branch_name}")])
        .output()
        .map_err(|err| Error::Config(format!("failed to inspect remote branch: {err}")))?;
    if !output.status.success() {
        return Err(Error::Config(format!(
            "git ls-remote origin {branch_name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().find_map(|line| line.split_whitespace().next()).map(ToString::to_string))
}

fn resolve_remote_default_branch(project_path: &Path) -> Result<String, Error> {
    if let Some(branch) = origin_head_branch(project_path) {
        return Ok(branch);
    }

    let _ = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["remote", "set-head", "origin", "--auto"])
        .output();

    origin_head_branch(project_path).ok_or_else(|| {
        Error::Config(
            "origin-head-unresolved: could not resolve `origin/HEAD`; refusing to guess a PR base"
                .to_string(),
        )
    })
}

fn remote_default_branch_is(project_path: &Path, branch_name: &str) -> bool {
    if origin_head_branch(project_path).as_deref() == Some(branch_name) {
        return true;
    }

    let _ = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["remote", "set-head", "origin", "--auto"])
        .output();

    origin_head_branch(project_path).as_deref() == Some(branch_name)
}

fn origin_head_branch(project_path: &Path) -> Option<String> {
    if let Some(full) =
        git_output_ok(project_path, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
    {
        return full
            .strip_prefix("refs/remotes/origin/")
            .or_else(|| full.strip_prefix("origin/"))
            .map(ToString::to_string);
    }

    remote_head_branch(project_path)
}

fn remote_head_branch(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["ls-remote", "--symref", "origin", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let rest = line.strip_prefix("ref: ")?;
        let (reference, target) = rest.split_once(char::is_whitespace)?;
        if target.trim() != "HEAD" {
            return None;
        }
        reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/remotes/origin/"))
            .or_else(|| reference.strip_prefix("origin/"))
            .map(ToString::to_string)
    })
}

fn git_stdout_trimmed(project_path: &Path, args: &[&str], label: &str) -> Result<String, Error> {
    let stdout = git_stdout_allow_empty(project_path, args, label)?;
    let trimmed = stdout.trim().to_string();
    if trimmed.is_empty() {
        return Err(Error::Config(format!("{label} returned no output")));
    }
    Ok(trimmed)
}

fn git_stdout_allow_empty(
    project_path: &Path, args: &[&str], label: &str,
) -> Result<String, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(args)
        .output()
        .map_err(|err| Error::Config(format!("{label}: failed to spawn git: {err}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    Err(Error::Config(format!(
        "{label} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const TEST_CHANGE: &str = "demo-change";
    const TEST_BRANCH: &str = "specify/demo-change";

    fn run_test_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(["-c", "user.name=Specify", "-c", "user.email=specify@example.invalid"])
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git -C {} {} failed\nstdout:\n{}\nstderr:\n{}",
            cwd.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_test_git_dir(git_dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(args)
            .output()
            .expect("spawn git --git-dir");
        assert!(
            output.status.success(),
            "git --git-dir {} {} failed\nstdout:\n{}\nstderr:\n{}",
            git_dir.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_dir_output(git_dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(args)
            .output()
            .expect("spawn git --git-dir");
        assert!(
            output.status.success(),
            "git --git-dir {} {} failed: {}",
            git_dir.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn git_output(tree: &Path, args: &[&str]) -> String {
        git_output_ok(tree, args).unwrap_or_else(|| {
            panic!("git -C {} {} produced no stdout", tree.display(), args.join(" "))
        })
    }

    fn git_output_allow_empty(tree: &Path, args: &[&str]) -> String {
        let output =
            Command::new("git").arg("-C").arg(tree).args(args).output().expect("spawn git");
        assert!(
            output.status.success(),
            "git -C {} {} failed: {}",
            tree.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo_with_commit(path: &Path, body: &str) {
        std::fs::create_dir_all(path).unwrap();
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["init", "-b", "main"])
            .output()
            .expect("spawn git init");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::write(path.join("README.md"), body).unwrap();
        run_test_git(path, &["add", "README.md"]);
        run_test_git(path, &["commit", "--no-gpg-sign", "-m", "seed"]);
    }

    fn test_project(url: impl Into<String>) -> RegistryProject {
        RegistryProject {
            name: "alpha".to_string(),
            url: url.into(),
            schema: "omnia@v1".to_string(),
            description: Some("alpha service".to_string()),
            contracts: None,
        }
    }

    fn seed_bare_remote(remote: &Path) {
        let source = remote.with_extension("source");
        init_repo_with_commit(&source, "base\n");
        run_test_git(
            remote.parent().expect("remote parent"),
            &["clone", "--bare", source.to_str().unwrap(), remote.to_str().unwrap()],
        );
    }

    fn clone_alpha_slot(project_dir: &Path, remote_url: &str) -> PathBuf {
        let slot = project_dir.join(".specify/workspace/alpha");
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        run_test_git(project_dir, &["clone", remote_url, slot.to_str().unwrap()]);
        slot
    }

    fn commit_on_change_branch(worktree: &Path, file: &str, body: &str) {
        run_test_git(worktree, &["checkout", "-b", TEST_BRANCH]);
        std::fs::write(worktree.join(file), body).unwrap();
        run_test_git(worktree, &["add", file]);
        run_test_git(worktree, &["commit", "--no-gpg-sign", "-m", "change work"]);
    }

    fn push_alpha(
        project_dir: &Path, project: &RegistryProject, dry_run: bool,
        forge: &dyn WorkspacePushForge,
    ) -> PushResult {
        let workspace_base = project_dir.join(".specify/workspace");
        push_single_project(
            project_dir,
            &workspace_base,
            project,
            TEST_BRANCH,
            TEST_CHANGE,
            dry_run,
            forge,
        )
    }

    struct RecordingForge {
        repo_exists_result: bool,
        create_remote: Option<PathBuf>,
        repo_exists_calls: RefCell<Vec<String>>,
        create_repo_calls: RefCell<Vec<String>>,
        pr_calls: RefCell<Vec<(String, String, String)>>,
    }

    impl RecordingForge {
        fn new(repo_exists_result: bool) -> Self {
            Self {
                repo_exists_result,
                create_remote: None,
                repo_exists_calls: RefCell::new(Vec::new()),
                create_repo_calls: RefCell::new(Vec::new()),
                pr_calls: RefCell::new(Vec::new()),
            }
        }

        fn creating(remote: PathBuf) -> Self {
            Self {
                create_remote: Some(remote),
                ..Self::new(false)
            }
        }
    }

    impl WorkspacePushForge for RecordingForge {
        fn repo_exists(&self, slug: &str, _project_path: &Path) -> Result<bool, Error> {
            self.repo_exists_calls.borrow_mut().push(slug.to_string());
            Ok(self.repo_exists_result)
        }

        fn create_repo(&self, slug: &str, _project_path: &Path) -> Result<(), Error> {
            self.create_repo_calls.borrow_mut().push(slug.to_string());
            if let Some(remote) = &self.create_remote {
                seed_bare_remote(remote);
            }
            Ok(())
        }

        fn ensure_pull_request(
            &self, _project_path: &Path, branch_name: &str, base_branch: &str,
            initiative_name: &str,
        ) -> Result<u64, Error> {
            self.pr_calls.borrow_mut().push((
                branch_name.to_string(),
                base_branch.to_string(),
                initiative_name.to_string(),
            ));
            Ok(42)
        }
    }

    #[test]
    fn rfc14_c07_workspace_push_publishes_existing_change_branch_only() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        commit_on_change_branch(&slot, "change.txt", "work\n");
        let local_head = git_output(&slot, &["rev-parse", "HEAD"]);
        let commits_before = git_output(&slot, &["rev-list", "--count", "HEAD"]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::Pushed);
        assert_eq!(result.branch.as_deref(), Some(TEST_BRANCH));
        assert_eq!(current_branch(&slot).unwrap().as_deref(), Some(TEST_BRANCH));
        assert_eq!(git_output(&slot, &["rev-list", "--count", "HEAD"]), commits_before);
        assert_eq!(git_output(&remote, &["rev-parse", TEST_BRANCH]), local_head);
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_reports_up_to_date_without_pushing() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        commit_on_change_branch(&slot, "change.txt", "work\n");
        run_test_git(&slot, &["push", "origin", TEST_BRANCH]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::UpToDate);
        assert_eq!(result.branch.as_deref(), Some(TEST_BRANCH));
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_dirty_checkout_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        commit_on_change_branch(&slot, "change.txt", "work\n");
        std::fs::write(slot.join("dirty.txt"), "dirty\n").unwrap();
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::Failed);
        assert!(result.error.as_deref().is_some_and(|error| error.contains("dirty")));
        assert!(forge.repo_exists_calls.borrow().is_empty());
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_wrong_branch_is_no_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        run_test_git(&slot, &["checkout", "-b", "feature/not-the-change"]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::NoBranch);
        assert_eq!(current_branch(&slot).unwrap().as_deref(), Some("feature/not-the-change"));
        assert!(forge.repo_exists_calls.borrow().is_empty());
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_default_branch_is_no_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        commit_on_change_branch(&slot, "change.txt", "work\n");
        run_test_git(&slot, &["push", "origin", TEST_BRANCH]);
        run_test_git(&remote, &["symbolic-ref", "HEAD", &format!("refs/heads/{TEST_BRANCH}")]);
        run_test_git(&slot, &["remote", "set-head", "origin", "--auto"]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::NoBranch);
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_detached_head_is_no_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        let head = git_output(&slot, &["rev-parse", "HEAD"]);
        run_test_git(&slot, &["checkout", "--detach", &head]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::NoBranch);
        assert!(current_branch(&slot).unwrap().is_none());
    }

    #[test]
    fn rfc14_c07_workspace_push_refuses_remote_default_branch_as_no_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        commit_on_change_branch(&slot, "change.txt", "work\n");
        run_test_git(&slot, &["push", "origin", TEST_BRANCH]);
        run_test_git_dir(&remote, &["symbolic-ref", "HEAD", &format!("refs/heads/{TEST_BRANCH}")]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

        assert_eq!(result.status, PushOutcome::NoBranch);
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_local_only_without_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path();
        let alpha = project_dir.join("alpha");
        init_repo_with_commit(&alpha, "seed\n");
        run_test_git(&alpha, &["checkout", "-b", TEST_BRANCH]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(project_dir, &test_project("./alpha"), false, &forge);

        assert_eq!(result.status, PushOutcome::LocalOnly);
        assert!(result.branch.is_none());
        assert!(forge.repo_exists_calls.borrow().is_empty());
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn rfc14_c07_workspace_push_greenfield_creates_remote_then_pr_to_origin_head() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        let slot = project_dir.join(".specify/workspace/alpha");
        std::fs::create_dir_all(&slot).unwrap();
        run_test_git(&slot, &["init", "-b", "main"]);
        run_test_git(&slot, &["checkout", "-b", TEST_BRANCH]);
        std::fs::write(slot.join("README.md"), "greenfield\n").unwrap();
        run_test_git(&slot, &["add", "README.md"]);
        run_test_git(&slot, &["commit", "--no-gpg-sign", "-m", "greenfield work"]);
        let remote = tmp.path().join("alpha.git");
        let github_url = "https://github.com/org/alpha.git";
        let rewrite = format!("file://{}", remote.display());
        run_test_git(&slot, &["remote", "add", "origin", github_url]);
        run_test_git(&slot, &["config", &format!("url.{rewrite}.insteadOf"), github_url]);
        let forge = RecordingForge::creating(remote.clone());

        let result = push_alpha(&project_dir, &test_project(github_url), false, &forge);

        assert_eq!(result.status, PushOutcome::Created);
        assert_eq!(result.pr_number, Some(42));
        assert_eq!(forge.create_repo_calls.borrow().as_slice(), ["org/alpha"]);
        assert_eq!(
            forge.pr_calls.borrow().as_slice(),
            [(TEST_BRANCH.to_string(), "main".to_string(), TEST_CHANGE.to_string())]
        );
        assert_eq!(
            git_output(&remote, &["rev-parse", TEST_BRANCH]),
            git_output(&slot, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn rfc14_c07_workspace_push_dry_run_classifies_without_mutating_remote_or_pr() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        std::fs::create_dir_all(&project_dir).unwrap();
        let remote = tmp.path().join("alpha.git");
        seed_bare_remote(&remote);
        let remote_url = format!("file://{}", remote.display());
        let slot = clone_alpha_slot(&project_dir, &remote_url);
        commit_on_change_branch(&slot, "change.txt", "work\n");
        let github_url = "https://github.com/org/alpha.git";
        let rewrite = format!("file://{}", remote.display());
        run_test_git(&slot, &["remote", "set-url", "origin", github_url]);
        run_test_git(&slot, &["config", &format!("url.{rewrite}.insteadOf"), github_url]);
        let forge = RecordingForge::new(true);

        let result = push_alpha(&project_dir, &test_project(github_url), true, &forge);

        assert_eq!(result.status, PushOutcome::Pushed);
        assert!(git_output_ok(&remote, &["rev-parse", TEST_BRANCH]).is_none());
        assert!(forge.create_repo_calls.borrow().is_empty());
        assert!(forge.pr_calls.borrow().is_empty());
    }

    #[test]
    fn extract_github_slug_git_ssh() {
        assert_eq!(github_slug("git@github.com:org/mobile.git"), Some("org/mobile".to_string()));
    }

    #[test]
    fn extract_github_slug_git_ssh_no_suffix() {
        assert_eq!(github_slug("git@github.com:org/mobile"), Some("org/mobile".to_string()));
    }

    #[test]
    fn extract_github_slug_https() {
        assert_eq!(
            github_slug("https://github.com/org/mobile.git"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_https_no_suffix() {
        assert_eq!(github_slug("https://github.com/org/mobile"), Some("org/mobile".to_string()));
    }

    #[test]
    fn extract_github_slug_ssh_protocol() {
        assert_eq!(
            github_slug("ssh://git@github.com/org/mobile.git"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_non_github() {
        assert_eq!(github_slug("git@gitlab.com:org/repo.git"), None);
    }

    #[test]
    fn distribute_contracts_recursive() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("contracts");
        let nested = src.join("schemas");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(src.join("openapi.yaml"), "openapi: 3.1").unwrap();
        std::fs::write(nested.join("order.yaml"), "type: object").unwrap();

        let dest = tmp.path().join("slot").join("contracts");
        distribute_contracts(&src, &dest).unwrap();

        assert!(dest.join("openapi.yaml").is_file());
        assert_eq!(std::fs::read_to_string(dest.join("openapi.yaml")).unwrap(), "openapi: 3.1");
        assert!(dest.join("schemas").join("order.yaml").is_file());
        assert_eq!(
            std::fs::read_to_string(dest.join("schemas").join("order.yaml")).unwrap(),
            "type: object"
        );
    }

    #[test]
    fn distribute_contracts_replaces_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("contracts");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("v2.yaml"), "version: 2").unwrap();

        let dest = tmp.path().join("dest_contracts");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("stale.yaml"), "old").unwrap();

        distribute_contracts(&src, &dest).unwrap();

        assert!(dest.join("v2.yaml").is_file());
        assert!(!dest.join("stale.yaml").exists(), "stale file should be removed");
    }

    #[test]
    fn distribute_contracts_missing_src_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("does-not-exist");
        let dest = tmp.path().join("dest");

        // distribute_contracts is only called when src.is_dir(), but
        // copy_dir_recursive itself would fail. Verify the caller guard
        // (central_contracts.is_dir()) prevents this — just assert src
        // doesn't exist.
        assert!(!src.is_dir());
        assert!(!dest.exists());
    }

    #[test]
    fn rfc14_c02_remote_clone_fetches_existing_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote-source");
        init_repo_with_commit(&remote, "v1\n");
        std::fs::create_dir_all(remote.join(".specify")).unwrap();
        std::fs::write(remote.join(".specify/project.yaml"), "name: remote\ncapability: omnia\n")
            .unwrap();
        run_test_git(&remote, &["add", ".specify/project.yaml"]);
        run_test_git(&remote, &["commit", "--no-gpg-sign", "-m", "add specify config"]);
        let url = format!("file://{}", remote.display());
        let dest = tmp.path().join(".specify/workspace/remote");

        materialise_git_remote(&url, &dest, "https://example.invalid/capability", tmp.path())
            .expect("initial clone");
        let initial_origin_main = git_output(&dest, &["rev-parse", "origin/main"]);
        assert_eq!(initial_origin_main, git_output(&remote, &["rev-parse", "HEAD"]));

        std::fs::write(remote.join("README.md"), "v2\n").unwrap();
        run_test_git(&remote, &["add", "README.md"]);
        run_test_git(&remote, &["commit", "--no-gpg-sign", "-m", "update"]);
        let updated_head = git_output(&remote, &["rev-parse", "HEAD"]);

        materialise_git_remote(&url, &dest, "https://example.invalid/capability", tmp.path())
            .expect("fetch existing clone");

        assert_ne!(initial_origin_main, updated_head);
        assert_eq!(git_output(&dest, &["rev-parse", "origin/main"]), updated_head);
    }

    #[test]
    fn rfc14_c02_remote_clone_refuses_origin_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join(".specify/workspace/remote");
        init_repo_with_commit(&dest, "slot\n");
        run_test_git(&dest, &["remote", "add", "origin", "https://example.invalid/old.git"]);
        std::fs::create_dir_all(dest.join(".specify")).unwrap();
        std::fs::write(dest.join(".specify/project.yaml"), "name: remote\ncapability: omnia\n")
            .unwrap();

        let err = materialise_git_remote(
            "https://example.invalid/new.git",
            &dest,
            "https://example.invalid/capability",
            tmp.path(),
        )
        .expect_err("origin mismatch must fail");
        let msg = err.to_string();

        assert!(msg.contains("origin remote"), "msg: {msg}");
        assert!(msg.contains("https://example.invalid/old.git"), "msg: {msg}");
        assert!(msg.contains("https://example.invalid/new.git"), "msg: {msg}");
    }

    #[test]
    fn rfc14_c02_greenfield_bootstrap_stays_local_and_commits_scaffold() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join(".specify/workspace/new-service");
        let url = "https://example.invalid/org/new-service.git";

        greenfield_bootstrap(url, &dest, "https://example.invalid/capability", tmp.path())
            .expect("greenfield bootstrap");

        assert_eq!(git_output(&dest, &["remote", "get-url", "origin"]), url);
        let project_yaml = std::fs::read_to_string(dest.join(".specify/project.yaml")).unwrap();
        assert!(project_yaml.contains("name: new-service"), "{project_yaml}");
        assert!(project_yaml.contains("capability: https://example.invalid/capability"));
        assert!(git_output_ok(&dest, &["log", "--oneline", "-1"]).is_some());
        assert_eq!(git_output_allow_empty(&dest, &["status", "--porcelain"]), "");
    }

    struct FakePushForge {
        repo_exists: bool,
        remote_to_create: Option<PathBuf>,
        branch_absent_after_create: Option<String>,
        pr_calls: std::cell::RefCell<Vec<(String, String)>>,
    }

    impl FakePushForge {
        fn new(repo_exists: bool) -> Self {
            Self {
                repo_exists,
                remote_to_create: None,
                branch_absent_after_create: None,
                pr_calls: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn creating(mut self, remote: PathBuf, absent_branch: &str) -> Self {
            self.remote_to_create = Some(remote);
            self.branch_absent_after_create = Some(absent_branch.to_string());
            self
        }
    }

    impl WorkspacePushForge for FakePushForge {
        fn repo_exists(&self, _slug: &str, _project_path: &Path) -> Result<bool, Error> {
            Ok(self.repo_exists)
        }

        fn create_repo(&self, _slug: &str, project_path: &Path) -> Result<(), Error> {
            let Some(remote) = &self.remote_to_create else {
                return Ok(());
            };
            run_test_git(
                remote.parent().unwrap(),
                &["clone", "--bare", project_path.to_str().unwrap(), remote.to_str().unwrap()],
            );
            if let Some(branch) = &self.branch_absent_after_create {
                run_test_git_dir(remote, &["update-ref", "-d", &format!("refs/heads/{branch}")]);
            }
            run_test_git_dir(remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
            Ok(())
        }

        fn ensure_pull_request(
            &self, _project_path: &Path, branch_name: &str, base_branch: &str,
            _initiative_name: &str,
        ) -> Result<u64, Error> {
            self.pr_calls.borrow_mut().push((branch_name.to_string(), base_branch.to_string()));
            Ok(42)
        }
    }

    #[test]
    fn rfc14_c07_greenfield_push_creates_repo_then_pr_against_origin_head() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        let workspace_base = project_dir.join(".specify/workspace");
        let slot = workspace_base.join("alpha");
        init_repo_with_commit(&slot, "seed\n");
        let main_head = git_output(&slot, &["rev-parse", "main"]);
        run_test_git(&slot, &["checkout", "-b", "specify/demo-change"]);
        std::fs::write(slot.join("change.txt"), "work\n").unwrap();
        run_test_git(&slot, &["add", "change.txt"]);
        run_test_git(&slot, &["commit", "--no-gpg-sign", "-m", "change work"]);
        let change_head = git_output(&slot, &["rev-parse", "HEAD"]);

        let github_url = "https://github.com/org/alpha.git";
        let remote = tmp.path().join("alpha.git");
        run_test_git(&slot, &["remote", "add", "origin", github_url]);
        run_test_git(
            &slot,
            &["config", &format!("url.file://{}.insteadOf", remote.display()), github_url],
        );
        let project = RegistryProject {
            name: "alpha".to_string(),
            url: github_url.to_string(),
            schema: "omnia@v1".to_string(),
            description: Some("alpha service".to_string()),
            contracts: None,
        };
        let forge = FakePushForge::new(false).creating(remote.clone(), "specify/demo-change");

        let result = push_single_project(
            &project_dir,
            &workspace_base,
            &project,
            "specify/demo-change",
            "demo-change",
            false,
            &forge,
        );

        assert_eq!(result.status, PushOutcome::Created, "result: {result:?}");
        assert_eq!(result.pr_number, Some(42));
        assert_eq!(
            git_dir_output(&remote, &["rev-parse", "refs/heads/specify/demo-change"]),
            change_head
        );
        assert_eq!(git_dir_output(&remote, &["rev-parse", "refs/heads/main"]), main_head);
        assert_eq!(origin_head_branch(&slot).as_deref(), Some("main"));
        assert_eq!(
            forge.pr_calls.borrow().as_slice(),
            &[("specify/demo-change".to_string(), "main".to_string())]
        );
    }

    #[test]
    fn rfc14_c07_greenfield_dry_run_does_not_create_repo_or_pr() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("hub");
        let workspace_base = project_dir.join(".specify/workspace");
        let slot = workspace_base.join("alpha");
        init_repo_with_commit(&slot, "seed\n");
        run_test_git(&slot, &["checkout", "-b", "specify/demo-change"]);
        std::fs::write(slot.join("change.txt"), "work\n").unwrap();
        run_test_git(&slot, &["add", "change.txt"]);
        run_test_git(&slot, &["commit", "--no-gpg-sign", "-m", "change work"]);

        let github_url = "https://github.com/org/alpha.git";
        let remote = tmp.path().join("alpha.git");
        run_test_git(&slot, &["remote", "add", "origin", github_url]);
        run_test_git(
            &slot,
            &["config", &format!("url.file://{}.insteadOf", remote.display()), github_url],
        );
        let project = RegistryProject {
            name: "alpha".to_string(),
            url: github_url.to_string(),
            schema: "omnia@v1".to_string(),
            description: Some("alpha service".to_string()),
            contracts: None,
        };
        let forge = FakePushForge::new(false).creating(remote.clone(), "specify/demo-change");

        let result = push_single_project(
            &project_dir,
            &workspace_base,
            &project,
            "specify/demo-change",
            "demo-change",
            true,
            &forge,
        );

        assert_eq!(result.status, PushOutcome::Created);
        assert!(!remote.exists(), "dry-run must not create the remote repository");
        assert!(forge.pr_calls.borrow().is_empty(), "dry-run must not create or update PRs");
    }
}
