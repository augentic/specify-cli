//! Multi-project workspace materialisation under `.specify/workspace/`
//! (RFC-3a C29).

use std::path::Path;
use std::process::Command;

use specify_error::Error;
use specify_schema::Registry;

use crate::config::ProjectConfig;
use crate::init::ensure_specify_gitignore_entries;

/// Materialise `.specify/workspace/<name>/` for every registry entry:
/// symlinks for `.` / relative URLs, shallow `git clone` or `git fetch`
/// for remotes. Ensures `.gitignore` lists `.specify/workspace/` (and
/// `.specify/.cache/` when missing).
pub fn sync_registry_workspace(project_dir: &Path) -> Result<(), Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(());
    };

    ensure_specify_gitignore_entries(project_dir)?;

    let base = ProjectConfig::specify_dir(project_dir).join("workspace");
    std::fs::create_dir_all(&base)?;

    for project in &registry.projects {
        let dest = base.join(&project.name);
        if project.url_materialises_as_symlink() {
            materialise_symlink(project_dir, &project.url, &dest)?;
        } else {
            materialise_git_remote(&project.url, &dest)?;
        }
    }
    Ok(())
}

/// One row for `specify initiative workspace status` text/JSON output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSlotStatus {
    /// Registry project name (`.specify/workspace/<name>/`).
    pub name: String,
    /// How the slot is materialised on disk.
    pub kind: WorkspaceSlotKind,
    /// `git rev-parse HEAD` when the resolved tree is a git checkout.
    pub head_sha: Option<String>,
    /// `true` when `git status --porcelain` is non-empty.
    pub dirty: Option<bool>,
}

/// Classification of a workspace slot on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSlotKind {
    /// Path missing.
    Missing,
    /// Symlink under `.specify/workspace/<name>/`.
    Symlink,
    /// Ordinary directory with a `.git/` metadata tree (clone target).
    GitClone,
    /// Present but neither a recognised symlink nor a git work tree.
    Other,
}

/// Inspect `.specify/workspace/<name>/` for each registry project.
///
/// Returns `Ok(None)` when `.specify/registry.yaml` is absent.
pub fn workspace_status(project_dir: &Path) -> Result<Option<Vec<WorkspaceSlotStatus>>, Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(None);
    };

    let base = ProjectConfig::specify_dir(project_dir).join("workspace");
    let mut out = Vec::with_capacity(registry.projects.len());
    for project in &registry.projects {
        let slot = base.join(&project.name);
        out.push(describe_slot(&project.name, &slot));
    }
    Ok(Some(out))
}

fn describe_slot(name: &str, slot: &Path) -> WorkspaceSlotStatus {
    let meta = match std::fs::symlink_metadata(slot) {
        Ok(m) => m,
        Err(_) => {
            return WorkspaceSlotStatus {
                name: name.to_string(),
                kind: WorkspaceSlotKind::Missing,
                head_sha: None,
                dirty: None,
            };
        }
    };

    if meta.file_type().is_symlink() {
        let (head_sha, dirty) =
            if slot.exists() { git_head_and_dirty_for_tree(slot) } else { (None, None) };
        return WorkspaceSlotStatus {
            name: name.to_string(),
            kind: WorkspaceSlotKind::Symlink,
            head_sha,
            dirty,
        };
    }

    if meta.is_dir() && slot.join(".git").exists() {
        let (head_sha, dirty) = git_head_and_dirty_for_tree(slot);
        return WorkspaceSlotStatus {
            name: name.to_string(),
            kind: WorkspaceSlotKind::GitClone,
            head_sha,
            dirty,
        };
    }

    WorkspaceSlotStatus {
        name: name.to_string(),
        kind: WorkspaceSlotKind::Other,
        head_sha: None,
        dirty: None,
    }
}

fn git_head_and_dirty_for_tree(tree: &Path) -> (Option<String>, Option<bool>) {
    let head = git_output_ok(tree, &["rev-parse", "HEAD"]);
    let dirty = head.as_ref().map(|_| git_porcelain_non_empty(tree));
    (head, dirty)
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
    let output =
        match Command::new("git").arg("-C").arg(tree).args(["status", "--porcelain"]).output() {
            Ok(o) => o,
            Err(_) => return false,
        };
    if !output.status.success() {
        return false;
    }
    !output.stdout.is_empty()
}

fn materialise_symlink(project_dir: &Path, url: &str, dest: &Path) -> Result<(), Error> {
    let target = if url == "." {
        std::fs::canonicalize(project_dir).map_err(|e| {
            Error::Config(format!(
                "could not resolve project directory for registry url `.`: {}",
                e
            ))
        })?
    } else {
        let joined = project_dir.join(url);
        std::fs::canonicalize(&joined).map_err(|e| {
            Error::Config(format!(
                "could not resolve registry url `{url}` relative to {}: {}",
                project_dir.display(),
                e
            ))
        })?
    };

    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(dest) {
            Ok(resolved) if resolved == target => return Ok(()),
            Ok(_) => {
                return Err(Error::Config(format!(
                    ".specify/workspace/{} already exists as a symlink pointing elsewhere (expected {})",
                    dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    target.display()
                )));
            }
            Err(_) => {
                std::fs::remove_file(dest).map_err(Error::Io)?;
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
        Err(Error::Config(
            "platform does not support symlinks for `specify initiative workspace sync`".into(),
        ))
    }
}

fn materialise_git_remote(url: &str, dest: &Path) -> Result<(), Error> {
    if dest.exists() {
        if dest.join(".git").is_dir() {
            run_git(dest, &["fetch", "--depth", "1"], &format!("git fetch in {}", dest.display()))?;
            return Ok(());
        }
        return Err(Error::Config(format!(
            "`{}` exists but is not a git clone (no `.git/`); remove it or pick another registry name",
            dest.display()
        )));
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }

    let status = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(dest)
        .status()
        .map_err(|e| {
            Error::Config(format!(
                "failed to spawn `git clone` for registry url `{url}`: {e} (is `git` installed?)"
            ))
        })?;

    if !status.success() {
        return Err(Error::Config(format!(
            "`git clone --depth 1 {url} {}` failed (non-zero exit)",
            dest.display()
        )));
    }
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str], label: &str) -> Result<(), Error> {
    let output = Command::new("git")
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
