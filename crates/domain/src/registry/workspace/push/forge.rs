//! Forge-side surface of `specify workspace push` — `repo_exists`,
//! `create_repo`, and `ensure_pull_request` shell out to `gh` through
//! an explicit [`CmdRunner`] for testability.

use std::path::Path;
use std::process::Command;

use specify_error::Error;

use crate::cmd::CmdRunner;

/// Returns `Ok(true)` when `gh repo view <slug>` succeeds (the remote
/// repository exists and the user can see it), `Ok(false)` when `gh`
/// reports a 404/not-found and `Err` on any other shell-out failure.
///
/// # Errors
///
/// Surfaces forge errors verbatim as `Error::Diag` so the caller can
/// chain them into per-project `PushResult.error`.
pub(in crate::registry::workspace) fn repo_exists<R: CmdRunner>(
    runner: &R, slug: &str,
) -> Result<bool, Error> {
    let mut cmd = Command::new("gh");
    cmd.args(["repo", "view", slug, "--json", "name"]);
    let output = runner.run(&mut cmd).map_err(|err| Error::Diag {
        code: "workspace-gh-spawn-failed",
        detail: format!("failed to spawn `gh repo view`: {err}"),
    })?;
    if output.status.success() {
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("could not resolve") || lower.contains("404") {
        return Ok(false);
    }
    Err(Error::Diag {
        code: "workspace-gh-repo-view-failed",
        detail: format!("gh repo view {slug} failed: {}", stderr.trim()),
    })
}

/// Create the remote repository for `slug`. Invoked inside `project_path`
/// so `gh` picks up the worktree's git config.
///
/// # Errors
///
/// Returns `Error::Diag` when `gh repo create` fails for any reason —
/// already-exists, permission denied, network, etc.
pub(in crate::registry::workspace) fn create_repo<R: CmdRunner>(
    runner: &R, slug: &str, project_path: &Path,
) -> Result<(), Error> {
    let mut cmd = Command::new("gh");
    cmd.args(["repo", "create", slug, "--private", "--source", "."]).current_dir(project_path);
    let output = runner.run(&mut cmd).map_err(|err| Error::Diag {
        code: "workspace-gh-spawn-failed",
        detail: format!("failed to spawn `gh repo create`: {err}"),
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Diag {
        code: "workspace-gh-repo-create-failed",
        detail: format!(
            "gh repo create {slug} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    })
}

/// Ensure that a PR exists for `branch_name` against `base_branch`.
///
/// When an existing PR is found, retargets it onto `base_branch` (the
/// remote default may have changed since the original push). When no PR
/// exists, creates a fresh one titled `specify: <change_name>`.
///
/// # Errors
///
/// Returns `Error::Diag` for any failure in the underlying `gh pr list`
/// / `gh pr edit` / `gh pr create` flow, including the case where
/// `gh pr create` returns no PR number on success.
pub(in crate::registry::workspace) fn ensure_pull_request<R: CmdRunner>(
    runner: &R, project_path: &Path, branch_name: &str, base_branch: &str, change_name: &str,
) -> Result<u64, Error> {
    let existing = github_pr_for_branch(runner, project_path, branch_name)?;
    if let Some(number) = existing {
        let mut edit_cmd = Command::new("gh");
        edit_cmd
            .args(["pr", "edit", &number.to_string(), "--base", base_branch])
            .current_dir(project_path);
        let edit = runner.run(&mut edit_cmd).map_err(|err| Error::Diag {
            code: "workspace-gh-spawn-failed",
            detail: format!("failed to spawn `gh pr edit`: {err}"),
        })?;
        if edit.status.success() {
            return Ok(number);
        }
        return Err(Error::Diag {
            code: "workspace-gh-pr-edit-failed",
            detail: format!(
                "gh pr edit #{number} failed: {}",
                String::from_utf8_lossy(&edit.stderr).trim()
            ),
        });
    }

    let pr_title = format!("specify: {change_name}");
    let pr_body = format!(
        "Automated push from specify workspace push for change \
         `{change_name}`."
    );
    let mut create_cmd = Command::new("gh");
    create_cmd
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
        .current_dir(project_path);
    let create = runner.run(&mut create_cmd).map_err(|err| Error::Diag {
        code: "workspace-gh-spawn-failed",
        detail: format!("failed to spawn `gh pr create`: {err}"),
    })?;

    if !create.status.success() {
        return Err(Error::Diag {
            code: "workspace-gh-pr-create-failed",
            detail: format!(
                "gh pr create failed: {}",
                String::from_utf8_lossy(&create.stderr).trim()
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&create.stdout).trim().to_string();
    stdout.rsplit('/').next().and_then(|num| num.parse().ok()).ok_or_else(|| Error::Diag {
        code: "workspace-gh-pr-create-no-number",
        detail: format!("gh pr create returned no PR number: {stdout}"),
    })
}

fn github_pr_for_branch<R: CmdRunner>(
    runner: &R, project_path: &Path, branch_name: &str,
) -> Result<Option<u64>, Error> {
    let mut cmd = Command::new("gh");
    cmd.args([
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
    .current_dir(project_path);
    let output = runner.run(&mut cmd).map_err(|err| Error::Diag {
        code: "workspace-gh-spawn-failed",
        detail: format!("failed to spawn `gh pr list`: {err}"),
    })?;

    if !output.status.success() {
        return Err(Error::Diag {
            code: "workspace-gh-pr-list-failed",
            detail: format!(
                "gh pr list failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).map_err(|err| Error::Diag {
            code: "workspace-gh-pr-list-malformed",
            detail: format!("gh pr list returned invalid JSON: {err}"),
        })?;
    Ok(parsed.first().and_then(|pr| pr.get("number")).and_then(serde_json::Value::as_u64))
}
