//! GitHub PR + workspace cleanliness probes for `change finalize`.
//!
//! The orchestration in [`super::run`] is generic over [`Probe`] so unit
//! tests can substitute canned responses. The CLI binary plugs in
//! [`RealProbe`], which shells out to `gh pr view` and `git status`.

use std::path::Path;
use std::process::Command;

use crate::registry::RegistryProject;
use crate::registry::forge::{GhClient, PrState, PrView, RealGhClient, branches_match};

use super::{Landing, ProjectResult};

/// Abstraction over the external probes finalize depends on.
///
/// Two methods, one per guard's external side-effect: `pr_view_for_branch`
/// for the GitHub PR-state guard (delegates to `gh pr view`) and
/// `is_dirty` for the workspace-cleanliness guard (delegates to
/// `git status --porcelain`).
///
/// The CLI binary plugs in [`RealProbe`]; tests substitute a
/// mock that records calls and replays canned outputs. Both methods
/// operate **inside** `project_path` (i.e. the workspace clone, or
/// the source path for symlink-mode projects).
pub trait Probe {
    /// Look up the open/closed/merged PR for `branch`. Returns
    /// `Ok(None)` when no PR exists on that branch.
    ///
    /// # Errors
    ///
    /// Returns the underlying `gh pr view` stderr (or a parse-error
    /// message) verbatim when the shell-out fails or its JSON cannot
    /// be decoded. The string surfaces in the per-project `detail`.
    fn pr_view_for_branch(
        &self, project_path: &Path, branch: &str,
    ) -> Result<Option<PrView>, String>;

    /// Returns `true` when `git status --porcelain` for the workspace
    /// clone produces any output. Returns `false` when the path is not
    /// a git tree (e.g. a missing clone or a non-git directory) — the
    /// finalize guard treats "not a clone" as "nothing to refuse on";
    /// the PR-state guard owns the missing-clone case via `failed`.
    fn is_dirty(&self, project_path: &Path) -> bool;
}

/// Default [`Probe`] backed by `gh` + `git`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealProbe;

impl Probe for RealProbe {
    fn pr_view_for_branch(
        &self, project_path: &Path, branch: &str,
    ) -> Result<Option<PrView>, String> {
        RealGhClient.pr_view_for_branch(project_path, branch)
    }

    fn is_dirty(&self, project_path: &Path) -> bool {
        let Ok(output) = Command::new("git")
            .arg("-C")
            .arg(project_path)
            .args(["status", "--porcelain"])
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        !output.stdout.is_empty()
    }
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
pub(super) fn probe_one<P: Probe>(
    probe: &P, project_path: &Path, rp: &RegistryProject, expected_branch: &str, clean: bool,
) -> ProjectResult {
    let name = &rp.name;
    let pr_view = probe.pr_view_for_branch(project_path, expected_branch);

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
    let dirty = project_path.exists() && probe.is_dirty(project_path);

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
