//! Read-only forge helpers shared by workspace push/finalize. Discover
//! the PR for a `specify/<change>` branch, view its state, and verify
//! the head branch is exactly the branch Specify created.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::cmd::CmdRunner;
use crate::registry::catalog::RegistryProject;

/// Required prefix on a PR's `headRefName`.
pub const SPECIFY_BRANCH_PREFIX: &str = "specify/";

/// PR view as returned by `gh pr view --json state,merged,headRefName,number,url`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrView {
    /// `OPEN` / `CLOSED` / `MERGED`.
    pub state: PrState,
    /// `true` once a PR has landed; otherwise `false`.
    pub merged: bool,
    /// Branch name on the head side (the PR's source branch).
    #[serde(rename = "headRefName")]
    pub head_ref_name: String,
    /// PR number on the forge.
    pub number: u64,
    /// Permalink to the PR.
    pub url: String,
}

/// PR top-level state per the GitHub `GraphQL` contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    /// PR is open and awaiting action.
    Open,
    /// PR was closed without merging.
    Closed,
    /// PR was merged into base.
    Merged,
}

/// Look up the open/closed/merged PR for `branch` against `project_path`.
///
/// Discovers the PR number with `gh pr list --head <branch>` first, then
/// fetches the full state payload with `gh pr view <n>`. Both invocations
/// run inside `project_path` (the workspace clone, or the original source
/// path for symlink-mode projects).
///
/// # Errors
///
/// Returns the underlying `gh` stderr (or a parse-error message) verbatim
/// when the shell-out fails or when its JSON cannot be decoded.
pub fn pr_view_for_branch<R: CmdRunner>(
    runner: &R, project_path: &Path, branch: &str,
) -> Result<Option<PrView>, String> {
    let mut list_cmd = Command::new("gh");
    list_cmd
        .args([
            "pr", "list", "--head", branch, "--state", "all", "--json", "number", "--limit", "1",
        ])
        .current_dir(project_path);
    let list =
        runner.run(&mut list_cmd).map_err(|err| format!("failed to spawn `gh pr list`: {err}"))?;
    if !list.status.success() {
        let stderr = String::from_utf8_lossy(&list.stderr);
        return Err(format!("gh pr list failed: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&list.stdout);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout)
        .map_err(|err| format!("gh pr list returned invalid JSON: {err}"))?;
    let Some(pr) = parsed.first() else {
        return Ok(None);
    };
    let Some(number) = pr.get("number").and_then(serde_json::Value::as_u64) else {
        return Err(format!("gh pr list returned no `number` field: {stdout}"));
    };

    let mut view_cmd = Command::new("gh");
    view_cmd
        .args(["pr", "view", &number.to_string(), "--json", "state,merged,headRefName,number,url"])
        .current_dir(project_path);
    let view =
        runner.run(&mut view_cmd).map_err(|err| format!("failed to spawn `gh pr view`: {err}"))?;
    if !view.status.success() {
        let stderr = String::from_utf8_lossy(&view.stderr);
        return Err(format!("gh pr view failed: {stderr}"));
    }
    let view_stdout = String::from_utf8_lossy(&view.stdout);
    let pr_view: PrView = serde_json::from_str(&view_stdout)
        .map_err(|err| format!("gh pr view returned invalid JSON: {err}"))?;
    Ok(Some(pr_view))
}

/// Validate the literal `specify/<segment>` shape.
///
/// `<segment>` must be non-empty and contain no further `/`.
#[must_use]
pub fn is_specify_branch(branch: &str) -> bool {
    let Some(rest) = branch.strip_prefix(SPECIFY_BRANCH_PREFIX) else {
        return false;
    };
    !rest.is_empty() && !rest.contains('/')
}

/// Verify a PR's `headRefName` equals the resolved `specify/<change>` branch exactly.
#[must_use]
pub fn branches_match(head_ref_name: &str, expected_branch: &str) -> bool {
    is_specify_branch(expected_branch)
        && is_specify_branch(head_ref_name)
        && head_ref_name == expected_branch
}

/// Resolve the on-disk path for a workspace project.
///
/// Symlink projects use the original path (relative or `.`), everything
/// else lives under `.specify/workspace/<name>/`.
#[must_use]
pub fn project_path(project_dir: &Path, workspace_base: &Path, rp: &RegistryProject) -> PathBuf {
    if rp.is_local() {
        if rp.url == "." { project_dir.to_path_buf() } else { project_dir.join(&rp.url) }
    } else {
        workspace_base.join(&rp.name)
    }
}
