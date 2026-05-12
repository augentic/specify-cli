//! GitHub PR + workspace cleanliness probes for `change finalize`.
//! Generic over [`CmdRunner`] so unit tests can substitute canned
//! responses; the CLI binary plugs in `RealCmd`.

use std::path::Path;
use std::process::Command;

use super::{Landing, ProjectResult};
use crate::cmd::CmdRunner;
use crate::registry::RegistryProject;
use crate::registry::forge::{PrState, PrView, branches_match, pr_view_for_branch};

/// Returns `true` when `git status --porcelain` for `project_path` produces
/// any output.
///
/// Returns `false` when the path is not a git tree (e.g. a missing clone or
/// a non-git directory) — the finalize guard treats "not a clone" as
/// "nothing to refuse on"; the PR-state guard owns the missing-clone case
/// via `failed`.
pub fn is_dirty<R: CmdRunner>(runner: &R, project_path: &Path) -> bool {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(project_path).args(["status", "--porcelain"]);
    let Ok(output) = runner.run(&mut cmd) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    !output.stdout.is_empty()
}

/// Classify a single project's PR state without performing any
/// mutation.
///
/// Pure: takes the gh observations and the expected branch, returns a
/// [`Landing`] from the PR-state half of the universe. The caller
/// layers in dirtiness via [`combine`].
///
/// 1. No PR ⇒ `no-branch`.
/// 2. `headRefName` mismatch ⇒ `branch-pattern-mismatch`.
/// 3. Already merged ⇒ `merged`.
/// 4. Closed without merge ⇒ `closed`.
/// 5. Open ⇒ `unmerged`.
#[must_use]
pub fn classify_pr(pr: Option<&PrView>, expected_branch: &str) -> Landing {
    let Some(pr) = pr else {
        return Landing::NoBranch;
    };
    if !branches_match(&pr.head_ref_name, expected_branch) {
        return Landing::BranchPatternMismatch;
    }
    if pr.merged || matches!(pr.state, PrState::Merged) {
        return Landing::Merged;
    }
    if matches!(pr.state, PrState::Closed) {
        return Landing::Closed;
    }
    Landing::Unmerged
}

/// Combine the PR-state classification with the dirty-clone observation.
///
/// `dirty` overrides any non-failure PR state — a dirty clone is fatal
/// regardless of the PR's status, because a subsequent `--clean` would
/// drop the uncommitted work. `Failed` (gh shell error) takes precedence
/// over `Dirty` so the operator can see why the probe broke.
#[must_use]
pub const fn combine(pr_status: Landing, dirty: bool) -> Landing {
    if matches!(pr_status, Landing::Failed) {
        return Landing::Failed;
    }
    if dirty {
        return Landing::Dirty;
    }
    pr_status
}

/// Probe a single project — combine PR state and dirty observation
/// into one [`ProjectResult`] row.
pub(super) fn probe_one<R: CmdRunner>(
    runner: &R, project_path: &Path, rp: &RegistryProject, expected_branch: &str, clean: bool,
) -> ProjectResult {
    let name = &rp.name;
    let pr_view = pr_view_for_branch(runner, project_path, expected_branch);

    let (pr_status, pr_number, url, head_ref_name, pr_detail) = match pr_view {
        Ok(None) => (Landing::NoBranch, None, None, None, None::<String>),
        Ok(Some(pr)) => {
            let status = classify_pr(Some(&pr), expected_branch);
            let detail = match status {
                Landing::BranchPatternMismatch => Some(format!(
                    "PR #{} headRefName `{}` does not match expected branch `{}`; recreate or retarget the PR before finalizing",
                    pr.number, pr.head_ref_name, expected_branch
                )),
                Landing::Closed => Some(format!(
                    "PR #{} is CLOSED without merge; reopen or push a replacement and operator-merge before finalizing",
                    pr.number,
                )),
                Landing::Unmerged => Some(format!(
                    "PR #{} is still OPEN; operator-merge it through the forge UI or `gh pr merge`, then re-run `specify change finalize`",
                    pr.number
                )),
                _ => None,
            };
            (status, Some(pr.number), Some(pr.url), Some(pr.head_ref_name), detail)
        }
        Err(err) => (Landing::Failed, None, None, None, Some(err)),
    };

    // Only check porcelain when the path actually exists — `gh` will
    // already have failed (and produced `Failed`) for a missing clone.
    let dirty = project_path.exists() && is_dirty(runner, project_path);

    let final_status = combine(pr_status, dirty);

    let detail = match final_status {
        Landing::Dirty => Some(if clean {
            format!(
                "workspace clone `{}` has uncommitted work; refusing — `--clean` would drop those changes. \
                 Commit/push or stash, then re-run.",
                project_path.display()
            )
        } else {
            format!(
                "workspace clone `{}` has uncommitted work; refusing. \
                 Triage with `specify workspace status`, then re-run.",
                project_path.display()
            )
        }),
        _ => pr_detail,
    };

    ProjectResult {
        name: name.clone(),
        status: final_status,
        pr_number,
        url,
        head_ref_name,
        dirty: Some(dirty),
        detail,
    }
}
