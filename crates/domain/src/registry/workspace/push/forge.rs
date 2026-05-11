//! Forge-side surface of `specify workspace push`: the trait used by
//! orchestration plus a default `gh`-shelled implementation.

use std::path::Path;
use std::process::Command;

use specify_error::Error;

pub(in crate::registry::workspace) trait WorkspacePushForge {
    fn repo_exists(&self, slug: &str, project_path: &Path) -> Result<bool, Error>;
    fn create_repo(&self, slug: &str, project_path: &Path) -> Result<(), Error>;
    fn ensure_pull_request(
        &self, project_path: &Path, branch_name: &str, base_branch: &str, change_name: &str,
    ) -> Result<u64, Error>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct RealWorkspacePushForge;

impl WorkspacePushForge for RealWorkspacePushForge {
    fn repo_exists(&self, slug: &str, _project_path: &Path) -> Result<bool, Error> {
        let output = Command::new("gh")
            .args(["repo", "view", slug, "--json", "name"])
            .output()
            .map_err(|err| Error::Diag {
                code: "workspace-gh-spawn-failed",
                detail: format!("failed to spawn `gh repo view`: {err}"),
            })?;
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
        Err(Error::Diag {
            code: "workspace-gh-repo-view-failed",
            detail: format!("gh repo view {slug} failed: {}", stderr.trim()),
        })
    }

    fn create_repo(&self, slug: &str, project_path: &Path) -> Result<(), Error> {
        let output = Command::new("gh")
            .args(["repo", "create", slug, "--private", "--source", "."])
            .current_dir(project_path)
            .output()
            .map_err(|err| Error::Diag {
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

    fn ensure_pull_request(
        &self, project_path: &Path, branch_name: &str, base_branch: &str, change_name: &str,
    ) -> Result<u64, Error> {
        let existing = github_pr_for_branch(project_path, branch_name)?;
        if let Some(number) = existing {
            let edit = Command::new("gh")
                .args(["pr", "edit", &number.to_string(), "--base", base_branch])
                .current_dir(project_path)
                .output()
                .map_err(|err| Error::Diag {
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
            .map_err(|err| Error::Diag {
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
        .map_err(|err| Error::Diag {
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
