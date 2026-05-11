//! `specify workspace push`: publish slot work back to remote forges and
//! ensure (or update) the matching pull request.

mod forge;
pub(in crate::registry::workspace) mod remote;

use std::path::Path;

use specify_error::Error;

use self::forge::RealWorkspacePushForge;
pub(in crate::registry::workspace) use self::forge::WorkspacePushForge;
use self::remote::{
    RemoteBranchState, current_branch, ensure_pr_base_resolves_if_supported,
    ensure_pr_if_supported, inspect_remote_branch, is_git_worktree, remote_default_branch_is,
};
use super::git::{self, git_output_ok, git_status_porcelain, git_stdout_trimmed};
use super::workspace_base;
use crate::registry::Registry;
use crate::registry::forge::project_path;
use crate::registry::registry::RegistryProject;

crate::kebab_enum! {
    /// Classification of a single project push outcome.
    #[derive(Debug)]
    pub enum PushOutcome {
        /// Branch pushed to remote.
        Pushed => "pushed",
        /// Remote repo was created, then pushed.
        Created => "created",
        /// Push failed (see `PushResult.error`).
        Failed => "failed",
        /// No changes to push.
        UpToDate => "up-to-date",
        /// Local-only project (no remote configured).
        LocalOnly => "local-only",
        /// Checkout is not currently on the expected `specify/<change>` branch.
        NoBranch => "no-branch",
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
/// `change_name` is `plan.name` from the binary side; the registry
/// crate cannot depend on `specify-slice` (which already depends on
/// `specify-registry`), so callers flatten the field at the boundary.
///
/// # Errors
///
/// Surfaces unknown selectors from `Registry::select` before any per-project
/// push work runs.
pub fn push_all(
    project_dir: &Path, change_name: &str, registry: &Registry, filter_projects: &[String],
    dry_run: bool,
) -> Result<Vec<PushResult>, Error> {
    let target_projects = registry.select(filter_projects)?;
    push_projects(project_dir, change_name, &target_projects, dry_run)
}

/// Core implementation of `specify workspace push` for pre-resolved projects.
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
    let real_forge = RealWorkspacePushForge;

    let mut results = Vec::new();

    for rp in target_projects {
        let result = push_single_project(
            project_dir,
            &workspace_base,
            rp,
            &branch_name,
            change_name,
            dry_run,
            &real_forge,
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

#[expect(
    clippy::too_many_lines,
    reason = "Per-project push driver inlines the dirty/clone/branch/push pipeline so each step's failure mode stays local."
)]
pub(in crate::registry::workspace) fn push_single_project(
    project_dir: &Path, workspace_base: &Path, rp: &RegistryProject, branch_name: &str,
    change_name: &str, dry_run: bool, forge: &dyn WorkspacePushForge,
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
            &project_path,
            slug.as_deref(),
            branch_name,
            change_name,
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
        &project_path,
        slug.as_deref(),
        branch_name,
        change_name,
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
