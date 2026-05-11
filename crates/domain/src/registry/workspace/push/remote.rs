//! Remote-branch inspection helpers for `specify workspace push`: tip
//! discovery, default-branch resolution, and PR-base preflight.

use std::path::Path;
use std::process::Command;

use specify_error::Error;

use super::forge::WorkspacePushForge;
use crate::registry::workspace::git::git_output_ok;

pub(super) enum RemoteBranchState {
    Present(String),
    Absent,
    RepositoryMissing,
}

pub(super) fn is_git_worktree(project_path: &Path) -> bool {
    git_output_ok(project_path, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true")
}

pub(in crate::registry::workspace) fn current_branch(project_path: &Path) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|err| Error::Diag {
            code: "workspace-git-current-branch-failed",
            detail: format!("failed to inspect current branch: {err}"),
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!branch.is_empty()).then_some(branch))
}

pub(super) fn inspect_remote_branch(
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

pub(super) fn ensure_pr_if_supported(
    project_path: &Path, slug: Option<&str>, branch_name: &str, change_name: &str,
    forge: &dyn WorkspacePushForge,
) -> Result<Option<u64>, Error> {
    if slug.is_none() {
        return Ok(None);
    }
    let base_branch = resolve_remote_default_branch(project_path)?;
    if base_branch == branch_name {
        return Err(Error::Diag {
            code: "workspace-pr-base-equals-branch",
            detail: format!(
                "remote default branch resolves to `{branch_name}`; refusing to create a PR against \
                 its own head branch"
            ),
        });
    }
    forge.ensure_pull_request(project_path, branch_name, &base_branch, change_name).map(Some)
}

pub(super) fn ensure_pr_base_resolves_if_supported(
    project_path: &Path, slug: Option<&str>, branch_name: &str,
) -> Result<(), Error> {
    if slug.is_some() {
        let base_branch = resolve_remote_default_branch(project_path)?;
        if base_branch == branch_name {
            return Err(Error::Diag {
                code: "workspace-pr-base-equals-branch",
                detail: format!(
                    "remote default branch resolves to `{branch_name}`; refusing to treat it as a \
                     workspace push branch"
                ),
            });
        }
    }
    Ok(())
}

pub(super) fn remote_default_branch_is(project_path: &Path, branch_name: &str) -> bool {
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

fn remote_branch_head(project_path: &Path, branch_name: &str) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["ls-remote", "--heads", "origin", &format!("refs/heads/{branch_name}")])
        .output()
        .map_err(|err| Error::Diag {
            code: "workspace-git-ls-remote-spawn-failed",
            detail: format!("failed to inspect remote branch: {err}"),
        })?;
    if !output.status.success() {
        return Err(Error::Diag {
            code: "workspace-git-ls-remote-failed",
            detail: format!(
                "git ls-remote origin {branch_name} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
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

    origin_head_branch(project_path).ok_or_else(|| Error::Diag {
        code: "workspace-origin-head-unresolved",
        detail:
            "origin-head-unresolved: could not resolve `origin/HEAD`; refusing to guess a PR base"
                .to_string(),
    })
}

pub(in crate::registry::workspace) fn origin_head_branch(project_path: &Path) -> Option<String> {
    if let Some(full) =
        git_output_ok(project_path, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
    {
        return full
            .strip_prefix("refs/remotes/origin/")
            .or_else(|| full.strip_prefix("origin/"))
            .map(ToString::to_string);
    }

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
