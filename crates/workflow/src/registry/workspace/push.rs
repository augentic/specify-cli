//! `specrun workspace push`: publish slot work back to remote forges and
//! ensure (or update) the matching pull request.

mod forge;
pub(in crate::registry::workspace) mod remote;

use std::path::Path;

use specify_error::Error;

use self::remote::{
    RemoteBranchState, current_branch, ensure_pr_base_resolves_if_supported,
    ensure_pr_if_supported, inspect_remote_branch, is_git_worktree, remote_default_branch_is,
};
use super::git::{self, git_output_ok, git_status_porcelain, git_stdout_trimmed};
use super::workspace_base;
use crate::cmd::{CmdRunner, real_cmd};
use crate::registry::catalog::RegistryProject;
use crate::registry::forge::project_path;

/// Classification of a single project push outcome.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
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

/// Result of a per-project push operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PushResult {
    /// Registry project name.
    pub name: String,
    /// Outcome of this push.
    pub status: PushOutcome,
    /// Git branch pushed to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `GitHub` PR number when one was created or found.
    #[serde(rename = "pr", skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    /// Human-readable error when the push failed.
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// Core implementation of `specrun workspace push` for pre-resolved projects.
///
/// # Errors
///
/// Per-project push outcomes are returned in the result vector; the outer
/// `Result` is reserved for fatal errors that prevent any push from running.
pub fn push_projects(
    project_dir: &Path, change_name: &str, target_projects: &[&RegistryProject], dry_run: bool,
) -> Result<Vec<PushResult>, Error> {
    let branch_name = format!("specify/{change_name}");
    let workspace_base = workspace_base(project_dir);
    let runner: CmdRunner<'_> = &real_cmd;

    let mut results = Vec::new();

    for rp in target_projects {
        let result = push_single_project(
            project_dir,
            &workspace_base,
            rp,
            &branch_name,
            change_name,
            dry_run,
            runner,
        );
        results.push(result);
    }

    Ok(results)
}

fn push_branch(
    project_path: &Path, branch_name: &str, expected_remote_head: Option<&str>,
) -> Result<(), Error> {
    let lease = expected_remote_head.map_or_else(
        || format!("--force-with-lease=refs/heads/{branch_name}:"),
        |sha| format!("--force-with-lease=refs/heads/{branch_name}:{sha}"),
    );
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
    git::run(
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

struct Ready<'a> {
    rp: &'a RegistryProject,
    project_path: std::path::PathBuf,
    branch_name: &'a str,
    slug: Option<String>,
    local_head: String,
    remote_branch: RemoteBranchState,
    remote_head: Option<String>,
}

pub(in crate::registry::workspace) fn push_single_project(
    project_dir: &Path, workspace_base: &Path, rp: &RegistryProject, branch_name: &str,
    change_name: &str, dry_run: bool, runner: CmdRunner<'_>,
) -> PushResult {
    let ready = match prepare_push(project_dir, workspace_base, rp, branch_name, runner) {
        Ok(r) => r,
        Err(result) => return result,
    };
    publish_push(ready, dry_run, runner, change_name)
}

fn prepare_push<'a>(
    project_dir: &Path, workspace_base: &Path, rp: &'a RegistryProject, branch_name: &'a str,
    runner: CmdRunner<'_>,
) -> Result<Ready<'a>, PushResult> {
    let project_path = project_path(project_dir, workspace_base, rp);

    if !is_git_worktree(&project_path) {
        return Err(push_result(
            rp,
            PushOutcome::Failed,
            None,
            None,
            Some(format!("no git worktree found at {}", project_path.display())),
        ));
    }

    let Some(remote_url) = git_output_ok(&project_path, &["remote", "get-url", "origin"]) else {
        return Err(push_result(rp, PushOutcome::LocalOnly, None, None, None));
    };
    let forge_url = git_output_ok(&project_path, &["config", "--get", "remote.origin.url"])
        .unwrap_or(remote_url);

    match current_branch(&project_path) {
        Ok(Some(current)) if current == branch_name => {}
        Ok(_) => return Err(push_result(rp, PushOutcome::NoBranch, None, None, None)),
        Err(err) => {
            return Err(push_result(rp, PushOutcome::Failed, None, None, Some(err.to_string())));
        }
    }

    match git_status_porcelain(&project_path) {
        Ok(status) if status.is_empty() => {}
        Ok(_) => {
            return Err(push_result(
                rp,
                PushOutcome::Failed,
                Some(branch_name),
                None,
                Some("checkout is dirty; commit or clean local work before workspace push".into()),
            ));
        }
        Err(err) => {
            return Err(push_result(
                rp,
                PushOutcome::Failed,
                Some(branch_name),
                None,
                Some(err.to_string()),
            ));
        }
    }

    if remote_default_branch_is(&project_path, branch_name) {
        return Err(push_result(rp, PushOutcome::NoBranch, None, None, None));
    }

    let local_head =
        match git_stdout_trimmed(&project_path, &["rev-parse", "HEAD"], "rev-parse HEAD") {
            Ok(sha) => sha,
            Err(err) => {
                return Err(push_result(
                    rp,
                    PushOutcome::Failed,
                    Some(branch_name),
                    None,
                    Some(err.to_string()),
                ));
            }
        };

    let slug = github_slug(&forge_url);
    let remote_branch =
        match inspect_remote_branch(runner, &project_path, branch_name, slug.as_deref()) {
            Ok(state) => state,
            Err(err) => {
                return Err(push_result(
                    rp,
                    PushOutcome::Failed,
                    Some(branch_name),
                    None,
                    Some(err.to_string()),
                ));
            }
        };
    let remote_head = match &remote_branch {
        RemoteBranchState::Present(sha) => Some(sha.clone()),
        RemoteBranchState::Absent | RemoteBranchState::RepositoryMissing => None,
    };

    Ok(Ready {
        rp,
        project_path,
        branch_name,
        slug,
        local_head,
        remote_branch,
        remote_head,
    })
}

fn publish_push(
    ready: Ready<'_>, dry_run: bool, runner: CmdRunner<'_>, change_name: &str,
) -> PushResult {
    let Ready {
        rp,
        project_path,
        branch_name,
        slug,
        local_head,
        remote_branch,
        remote_head,
    } = ready;
    let remote_head_ref = remote_head.as_deref();

    if remote_head_ref == Some(local_head.as_str()) {
        return finish_up_to_date(
            runner,
            &project_path,
            rp,
            branch_name,
            change_name,
            slug.as_deref(),
            dry_run,
        );
    }

    let mut is_created = false;

    if matches!(remote_branch, RemoteBranchState::RepositoryMissing) {
        if dry_run {
            return push_result(rp, PushOutcome::Created, Some(branch_name), None, None);
        }
        if let Some(slug) = &slug {
            if let Err(err) = forge::create_repo(runner, slug, &project_path) {
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

    if let Err(e) = push_branch(&project_path, branch_name, remote_head_ref) {
        return push_result(rp, PushOutcome::Failed, Some(branch_name), None, Some(e.to_string()));
    }

    let pr_number = match ensure_pr_if_supported(
        runner,
        &project_path,
        slug.as_deref(),
        branch_name,
        change_name,
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

fn finish_up_to_date(
    runner: CmdRunner<'_>, project_path: &Path, rp: &RegistryProject, branch_name: &str,
    change_name: &str, slug: Option<&str>, dry_run: bool,
) -> PushResult {
    if dry_run {
        if let Err(err) = ensure_pr_base_resolves_if_supported(project_path, slug, branch_name) {
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
    ensure_pr_if_supported(runner, project_path, slug, branch_name, change_name).map_or_else(
        |err| push_result(rp, PushOutcome::Failed, Some(branch_name), None, Some(err.to_string())),
        |pr| push_result(rp, PushOutcome::UpToDate, Some(branch_name), pr, None),
    )
}
