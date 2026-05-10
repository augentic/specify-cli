//! Workspace status reporting: `SlotStatus` / `WorkspaceStatus`-shaped
//! output that powers `specify workspace status` and friends.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use specify_error::Error;

use super::git::{git_output_ok, git_porcelain_non_empty};
use super::{local_target_path, workspace_base};
use crate::Registry;
use crate::registry::RegistryProject;

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

/// Inspect `.specify/workspace/<name>/` for each registry project.
///
/// Returns `Ok(None)` when `.specify/registry.yaml` is absent.
///
/// # Errors
///
/// Surfaces registry parse failures from `Registry::load`.
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
