//! Read-only forge helpers shared by workspace push/finalize flows.
//!
//! Specify does not perform pull-request merging. This module keeps the
//! small PR inspection surface still needed by `change finalize`:
//! discover the PR for a `specify/<change>` branch, view its state, and
//! validate that the head branch is exactly the branch Specify created.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::registry::RegistryProject;

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

/// Abstraction over the read-only `gh` CLI subset we depend on.
pub trait GhClient {
    /// Look up the open/closed/merged PR for `branch`.
    ///
    /// Returns `Ok(None)` when no PR exists on that branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    fn pr_view_for_branch(
        &self, project_path: &Path, branch: &str,
    ) -> Result<Option<PrView>, String>;
}

/// Default [`GhClient`] backed by `Command::new("gh")`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealGhClient;

impl GhClient for RealGhClient {
    fn pr_view_for_branch(
        &self, project_path: &Path, branch: &str,
    ) -> Result<Option<PrView>, String> {
        // Discover by branch first, then view by number for the full
        // state payload. This is read-only and performs no forge mutation.
        let list = Command::new("gh")
            .args([
                "pr", "list", "--head", branch, "--state", "all", "--json", "number", "--limit",
                "1",
            ])
            .current_dir(project_path)
            .output()
            .map_err(|err| format!("failed to spawn `gh pr list`: {err}"))?;
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

        let view = Command::new("gh")
            .args([
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "state,merged,headRefName,number,url",
            ])
            .current_dir(project_path)
            .output()
            .map_err(|err| format!("failed to spawn `gh pr view`: {err}"))?;
        if !view.status.success() {
            let stderr = String::from_utf8_lossy(&view.stderr);
            return Err(format!("gh pr view failed: {stderr}"));
        }
        let view_stdout = String::from_utf8_lossy(&view.stdout);
        let pr_view: PrView = serde_json::from_str(&view_stdout)
            .map_err(|err| format!("gh pr view returned invalid JSON: {err}"))?;
        Ok(Some(pr_view))
    }
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
