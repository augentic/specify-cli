//! `specify workspace merge` — automated PR merging (RFC-9 §4A).
//!
//! Closes the upstream-landing half of the platform-first loop: for
//! every registry project that has an open PR on the
//! `specify/<initiative-name>` branch, check CI via `gh pr checks` and
//! squash-merge with `gh pr merge --squash` when all checks pass.
//! Best-effort across projects — a single project's failure surfaces
//! in the per-project status without aborting the others.
//!
//! ## Safety guards (non-negotiable per the brief)
//!
//! - **Branch-pattern guard.** Refuses to operate on any PR whose
//!   `headRefName` does not equal `specify/<initiative-name>`
//!   exactly. The literal expected branch is surfaced in
//!   `branch-pattern-mismatch` diagnostics so an operator can see the
//!   drift.
//! - **Never force-merge.** No `--admin`, no `--auto`, no override of
//!   failing checks. PRs with `FAILURE` checks classify as
//!   `failed-checks`; pending checks classify as `pending-checks`.
//! - **Never abort the batch.** Per-project failures (gh exec error,
//!   merge refusal, …) are recorded as `failed`/`failed-checks`/etc
//!   without stopping the loop.
//!
//! ## Testability
//!
//! Shell-out to `gh` lives behind the [`GhClient`] trait. The default
//! [`RealGhClient`] dispatches to `Command::new("gh")`; tests inject a
//! mock that records calls and replays canned `gh` outputs. The pure
//! [`classify_status`] function — invoked once per project — is the
//! only place per-project decision logic lives, which keeps the
//! state machine independently unit-testable.

#![allow(clippy::needless_pass_by_value)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::registry::{Registry, RegistryProject};

/// Absolute path to `<project_dir>/.specify/workspace/`. Mirror of
/// `ProjectConfig::specify_dir(...).join("workspace")` from the binary;
/// duplicated so the registry crate stays self-contained (the binary
/// is downstream of this crate, so we cannot reach back to
/// `ProjectConfig`).
fn workspace_base(project_dir: &Path) -> PathBuf {
    project_dir.join(".specify").join("workspace")
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-project classification for `specify workspace merge`.
///
/// Display strings are kebab-case and match the JSON `status` value
/// emitted by the CLI. Skill authors and operators rely on this
/// vocabulary; treat it as a stable wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStatus {
    /// No PR exists on the expected `specify/<initiative-name>` branch.
    NoBranch,
    /// A PR exists but its `headRefName` is not the expected branch.
    /// Defence in depth — our query filters by branch, but we still
    /// verify and refuse to operate when the result diverges.
    BranchPatternMismatch,
    /// The PR is already `MERGED` on the remote.
    Merged,
    /// The PR was `CLOSED` without merging.
    Closed,
    /// At least one CI check is in a failure bucket
    /// (`fail`/`cancel`/`skipping` are not all-pass).
    FailedChecks,
    /// At least one CI check is still running (`pending`).
    PendingChecks,
    /// Dry-run mode: would merge a PR with green CI.
    WouldMerge,
    /// Generic failure during shell-out (gh missing, unparseable
    /// JSON, network error, etc).
    Failed,
}

impl MergeStatus {
    /// Stable kebab-case identifier — the JSON wire value and the
    /// human-readable status column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoBranch => "no-branch",
            Self::BranchPatternMismatch => "branch-pattern-mismatch",
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::FailedChecks => "failed-checks",
            Self::PendingChecks => "pending-checks",
            Self::WouldMerge => "would-merge",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for MergeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-project result row, emitted in both text and JSON output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeProjectResult {
    /// Registry project name.
    pub name: String,
    /// Outcome of the merge attempt.
    pub status: MergeStatus,
    /// PR number when discovered (any state).
    pub pr_number: Option<u64>,
    /// PR URL when discovered (any state).
    pub url: Option<String>,
    /// `headRefName` reported by `gh pr view`. Surfaced in
    /// diagnostics for `branch-pattern-mismatch`.
    pub head_ref_name: Option<String>,
    /// Free-form context — gh stderr, parse errors, etc. Not part of
    /// the stable wire vocabulary but useful for operator triage.
    pub detail: Option<String>,
}

/// PR view as returned by `gh pr view --json state,merged,headRefName,number,url`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrView {
    /// `OPEN` / `CLOSED` / `MERGED`.
    pub state: PrState,
    /// `true` once a PR has landed; otherwise `false` (including when
    /// `state == CLOSED` without merge).
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

/// One CI check as returned by `gh pr checks --json bucket,name,state`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrCheck {
    /// `gh`'s coarse bucket: `pass`/`fail`/`pending`/`skipping`/`cancel`.
    /// We classify against the bucket rather than the workflow-specific
    /// `state` so we don't hard-code provider conventions.
    pub bucket: CheckBucket,
    /// Human-readable workflow / job name; surfaced in diagnostics.
    #[serde(default)]
    pub name: String,
}

/// `gh pr checks --json bucket` vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckBucket {
    /// All matching checks passed.
    Pass,
    /// At least one check failed.
    Fail,
    /// At least one check is still running.
    Pending,
    /// A check was explicitly skipped — neither pass nor fail.
    Skipping,
    /// A check run was cancelled — treat as failure.
    Cancel,
}

// ---------------------------------------------------------------------------
// GhClient trait — shell-out abstraction (testable seam per RFC-9 §4A brief)
// ---------------------------------------------------------------------------

/// Abstraction over the `gh` CLI subset we depend on.
///
/// The CLI binary plugs in [`RealGhClient`]; tests substitute a mock
/// that records calls and replays canned outputs. All methods operate
/// **inside** `project_path` (i.e. the workspace clone or the
/// initiating project itself for symlink-mode projects), so `gh` picks
/// up the correct remote and credentials.
pub trait GhClient {
    /// Look up the open/closed/merged PR for `branch`. Returns
    /// `Ok(None)` when no PR exists on that branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    fn pr_view_for_branch(
        &self, project_path: &Path, branch: &str,
    ) -> Result<Option<PrView>, String>;

    /// Inspect CI checks for a PR.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    fn pr_checks(&self, project_path: &Path, pr_number: u64) -> Result<Vec<PrCheck>, String>;

    /// Squash-merge a PR. Never force-merges; never overrides checks;
    /// never uses `--admin` — those flags are deliberately absent from
    /// the trait.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    fn pr_merge_squash(&self, project_path: &Path, pr_number: u64) -> Result<(), String>;
}

/// Default [`GhClient`] backed by `Command::new("gh")`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealGhClient;

impl GhClient for RealGhClient {
    fn pr_view_for_branch(
        &self, project_path: &Path, branch: &str,
    ) -> Result<Option<PrView>, String> {
        // Use `gh pr list --head <branch>` to discover the PR — the
        // branch-anchored shape matches `workspace push`'s discovery
        // path. Then promote to `pr view` for the full payload (state,
        // merged, headRefName, …).
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

    fn pr_checks(&self, project_path: &Path, pr_number: u64) -> Result<Vec<PrCheck>, String> {
        let output = Command::new("gh")
            .args(["pr", "checks", &pr_number.to_string(), "--json", "bucket,name"])
            .current_dir(project_path)
            .output()
            .map_err(|err| format!("failed to spawn `gh pr checks`: {err}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("gh pr checks failed: {stderr}"));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        // gh emits an empty `[]` when there are no checks.
        let parsed: Vec<PrCheck> = serde_json::from_str(&stdout)
            .map_err(|err| format!("gh pr checks returned invalid JSON: {err}"))?;
        Ok(parsed)
    }

    fn pr_merge_squash(&self, project_path: &Path, pr_number: u64) -> Result<(), String> {
        let output = Command::new("gh")
            .args(["pr", "merge", &pr_number.to_string(), "--squash"])
            .current_dir(project_path)
            .output()
            .map_err(|err| format!("failed to spawn `gh pr merge`: {err}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("gh pr merge failed: {stderr}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pure logic — branch matcher + classifier
// ---------------------------------------------------------------------------

/// Required prefix on a PR's `headRefName` for `workspace merge` to
/// consider it. Pinned as a constant so a typo on the `push` side and
/// the `merge` side cannot diverge.
pub const SPECIFY_BRANCH_PREFIX: &str = "specify/";

/// Validate the literal `specify/<segment>` shape.
///
/// `<segment>` must be non-empty and contain no further `/`. The set
/// of branches `workspace push` creates is exactly the family this
/// matcher accepts. Used by [`pr_branch_matches`] and exposed
/// publicly so tests and downstream tooling can share one validator.
///
/// Acceptance criteria:
/// - `specify/foo`, `specify/foo-bar`, `specify/foo-bar-1` accepted.
/// - `feature/bar`, `specify-foo`, `specify/foo/bar`, `specify/`,
///   the empty string — all rejected.
#[must_use]
pub fn matches_specify_branch_pattern(branch: &str) -> bool {
    let Some(rest) = branch.strip_prefix(SPECIFY_BRANCH_PREFIX) else {
        return false;
    };
    !rest.is_empty() && !rest.contains('/')
}

/// Verify a PR's `headRefName` equals the resolved
/// `specify/<initiative-name>` exactly. The brief calls this
/// non-negotiable — defence in depth even though our discovery query
/// already filters by branch.
#[must_use]
pub fn pr_branch_matches(head_ref_name: &str, expected_branch: &str) -> bool {
    matches_specify_branch_pattern(expected_branch)
        && matches_specify_branch_pattern(head_ref_name)
        && head_ref_name == expected_branch
}

/// Reduce a CI check list to a single status. Empty lists classify as
/// `Pass` — callers proceed to merge — matching `gh pr merge`'s own
/// posture when no protection rules are configured.
#[must_use]
pub fn classify_checks(checks: &[PrCheck]) -> CheckOverall {
    let mut any_pending = false;
    let mut any_fail = false;
    for c in checks {
        match c.bucket {
            CheckBucket::Pass | CheckBucket::Skipping => {}
            CheckBucket::Pending => any_pending = true,
            CheckBucket::Fail | CheckBucket::Cancel => any_fail = true,
        }
    }
    if any_fail {
        CheckOverall::Failing
    } else if any_pending {
        CheckOverall::Pending
    } else {
        CheckOverall::Passing
    }
}

/// Aggregate verdict from [`classify_checks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOverall {
    /// All checks passed (or the check list was empty).
    Passing,
    /// At least one check still running.
    Pending,
    /// At least one check failing or cancelled.
    Failing,
}

/// Classify a single project's PR state without performing the merge.
///
/// Pure: takes the gh observations and the expected branch, returns
/// a status. The caller wraps `Merged` vs `WouldMerge` based on the
/// `--dry-run` flag.
///
/// The argument shape mirrors the brief's algorithm:
///
/// 1. No PR ⇒ `no-branch`.
/// 2. `headRefName` mismatch ⇒ `branch-pattern-mismatch`.
/// 3. Already merged ⇒ `merged`.
/// 4. Closed ⇒ `closed`.
/// 5. Open + checks failing ⇒ `failed-checks`.
/// 6. Open + checks pending ⇒ `pending-checks`.
/// 7. Open + checks passing + `dry_run` ⇒ `would-merge`.
/// 8. Open + checks passing + not dry-run ⇒ caller invokes
///    `pr_merge_squash` and stamps `merged` or `failed`.
#[must_use]
pub fn classify_status(
    pr: Option<&PrView>, expected_branch: &str, checks: Option<&[PrCheck]>, dry_run: bool,
) -> MergeStatus {
    let Some(pr) = pr else {
        return MergeStatus::NoBranch;
    };
    if !pr_branch_matches(&pr.head_ref_name, expected_branch) {
        return MergeStatus::BranchPatternMismatch;
    }
    if pr.merged || matches!(pr.state, PrState::Merged) {
        return MergeStatus::Merged;
    }
    if matches!(pr.state, PrState::Closed) {
        return MergeStatus::Closed;
    }
    let overall = checks.map_or(CheckOverall::Passing, classify_checks);
    match overall {
        CheckOverall::Failing => MergeStatus::FailedChecks,
        CheckOverall::Pending => MergeStatus::PendingChecks,
        CheckOverall::Passing => {
            if dry_run {
                MergeStatus::WouldMerge
            } else {
                // Caller performs the actual merge and rewrites
                // `Merged`/`Failed` based on the gh result.
                MergeStatus::Merged
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Orchestration — generic over GhClient for testability
// ---------------------------------------------------------------------------

/// Resolve the on-disk path for a workspace project.
///
/// Mirrors the resolution rule in `workspace push` (RFC-3b): symlink
/// projects use the original path (relative or `.`), everything else
/// lives under `.specify/workspace/<name>/`.
#[must_use]
pub fn project_path_for(
    project_dir: &Path, workspace_base: &Path, rp: &RegistryProject,
) -> PathBuf {
    if rp.url_materialises_as_symlink() {
        if rp.url == "." { project_dir.to_path_buf() } else { project_dir.join(&rp.url) }
    } else {
        workspace_base.join(&rp.name)
    }
}

/// Core implementation of `specify workspace merge`.
///
/// `initiative_name` is `plan.name` from the binary side; the registry
/// crate cannot depend on `specify-change` (which already depends on
/// `specify-registry`), so callers flatten the field at the boundary.
///
/// # Errors
///
/// Returns an error before any per-project work when a precondition
/// fails (no plan, empty registry, …). Per-project errors are
/// captured in [`MergeProjectResult::status`] and never bubble up.
pub fn run_workspace_merge_impl<C: GhClient>(
    project_dir: &Path, initiative_name: &str, registry: &Registry, gh: &C,
    filter_projects: &[String], dry_run: bool,
) -> Result<Vec<MergeProjectResult>, Error> {
    if registry.projects.is_empty() {
        return Err(Error::Config(
            "registry.yaml has no projects; nothing to merge. \
             Add entries via `specify registry add`."
                .to_string(),
        ));
    }

    let branch_name = format!("{SPECIFY_BRANCH_PREFIX}{initiative_name}");
    let workspace_base = workspace_base(project_dir);

    let target_projects: Vec<&RegistryProject> = if filter_projects.is_empty() {
        registry.projects.iter().collect()
    } else {
        registry.projects.iter().filter(|p| filter_projects.contains(&p.name)).collect()
    };

    let mut results = Vec::with_capacity(target_projects.len());
    for rp in target_projects {
        let path = project_path_for(project_dir, &workspace_base, rp);
        results.push(merge_single_project(gh, &path, rp, &branch_name, dry_run));
    }
    Ok(results)
}

fn merge_single_project<C: GhClient>(
    gh: &C, project_path: &Path, rp: &RegistryProject, expected_branch: &str, dry_run: bool,
) -> MergeProjectResult {
    let name = &rp.name;
    let pr_view = match gh.pr_view_for_branch(project_path, expected_branch) {
        Ok(view) => view,
        Err(err) => return failed_with_detail(name, None, None, None, err),
    };
    let Some(pr) = pr_view else {
        return no_branch_for(name, expected_branch);
    };
    if !pr_branch_matches(&pr.head_ref_name, expected_branch) {
        return branch_mismatch_for(name, &pr, expected_branch);
    }
    if pr.merged || matches!(pr.state, PrState::Merged) {
        return terminal(name, MergeStatus::Merged, &pr, None);
    }
    if matches!(pr.state, PrState::Closed) {
        return terminal(
            name,
            MergeStatus::Closed,
            &pr,
            Some(format!("PR #{} is CLOSED without merge", pr.number)),
        );
    }
    classify_open_pr(gh, project_path, name, pr, dry_run)
}

/// Resolve the open-PR branch — checks, dry-run, real merge — once we
/// know the PR is `OPEN` and on the expected branch.
fn classify_open_pr<C: GhClient>(
    gh: &C, project_path: &Path, name: &str, pr: PrView, dry_run: bool,
) -> MergeProjectResult {
    let checks = match gh.pr_checks(project_path, pr.number) {
        Ok(checks) => checks,
        Err(err) => {
            return terminal(name, MergeStatus::Failed, &pr, Some(err));
        }
    };
    match classify_checks(&checks) {
        CheckOverall::Failing => {
            return terminal(
                name,
                MergeStatus::FailedChecks,
                &pr,
                Some(failing_check_detail(&checks)),
            );
        }
        CheckOverall::Pending => {
            return terminal(
                name,
                MergeStatus::PendingChecks,
                &pr,
                Some(pending_check_detail(&checks)),
            );
        }
        CheckOverall::Passing => {}
    }
    if dry_run {
        return terminal(name, MergeStatus::WouldMerge, &pr, None);
    }
    match gh.pr_merge_squash(project_path, pr.number) {
        Ok(()) => terminal(name, MergeStatus::Merged, &pr, None),
        Err(err) => terminal(name, MergeStatus::Failed, &pr, Some(err)),
    }
}

fn terminal(
    name: &str, status: MergeStatus, pr: &PrView, detail: Option<String>,
) -> MergeProjectResult {
    MergeProjectResult {
        name: name.to_string(),
        status,
        pr_number: Some(pr.number),
        url: Some(pr.url.clone()),
        head_ref_name: Some(pr.head_ref_name.clone()),
        detail,
    }
}

fn no_branch_for(name: &str, expected_branch: &str) -> MergeProjectResult {
    MergeProjectResult {
        name: name.to_string(),
        status: MergeStatus::NoBranch,
        pr_number: None,
        url: None,
        head_ref_name: None,
        detail: Some(format!(
            "no open PR on {expected_branch}; run `specify workspace push` first"
        )),
    }
}

fn branch_mismatch_for(name: &str, pr: &PrView, expected_branch: &str) -> MergeProjectResult {
    MergeProjectResult {
        name: name.to_string(),
        status: MergeStatus::BranchPatternMismatch,
        pr_number: Some(pr.number),
        url: Some(pr.url.clone()),
        head_ref_name: Some(pr.head_ref_name.clone()),
        detail: Some(format!(
            "PR #{} headRefName `{}` does not match expected branch `{}`; refusing to merge",
            pr.number, pr.head_ref_name, expected_branch
        )),
    }
}

fn failed_with_detail(
    name: &str, pr_number: Option<u64>, url: Option<String>, head_ref_name: Option<String>,
    detail: String,
) -> MergeProjectResult {
    MergeProjectResult {
        name: name.to_string(),
        status: MergeStatus::Failed,
        pr_number,
        url,
        head_ref_name,
        detail: Some(detail),
    }
}

fn failing_check_detail(checks: &[PrCheck]) -> String {
    let names: Vec<&str> = checks
        .iter()
        .filter(|c| matches!(c.bucket, CheckBucket::Fail | CheckBucket::Cancel))
        .map(|c| c.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        "one or more checks failed".to_string()
    } else {
        format!("failing checks: {}", names.join(", "))
    }
}

fn pending_check_detail(checks: &[PrCheck]) -> String {
    let names: Vec<&str> = checks
        .iter()
        .filter(|c| matches!(c.bucket, CheckBucket::Pending))
        .map(|c| c.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        "one or more checks are still running".to_string()
    } else {
        format!("pending checks: {}", names.join(", "))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    // ---- branch-pattern matcher ------------------------------------------

    #[test]
    fn branch_pattern_accepts_canonical_shape() {
        assert!(matches_specify_branch_pattern("specify/foo"));
        assert!(matches_specify_branch_pattern("specify/foo-bar"));
        assert!(matches_specify_branch_pattern("specify/platform-v2"));
        assert!(matches_specify_branch_pattern("specify/foo-bar-1"));
    }

    #[test]
    fn branch_pattern_rejects_other_prefixes() {
        assert!(!matches_specify_branch_pattern("feature/bar"));
        assert!(!matches_specify_branch_pattern("main"));
        assert!(!matches_specify_branch_pattern("specify-foo"));
    }

    #[test]
    fn branch_pattern_rejects_nested_segments() {
        assert!(!matches_specify_branch_pattern("specify/foo/bar"));
        assert!(!matches_specify_branch_pattern("specify/a/b/c"));
    }

    #[test]
    fn branch_pattern_rejects_empty_segment() {
        assert!(!matches_specify_branch_pattern(""));
        assert!(!matches_specify_branch_pattern("specify/"));
    }

    #[test]
    fn pr_branch_matches_requires_exact_equality() {
        assert!(pr_branch_matches("specify/foo", "specify/foo"));
        assert!(!pr_branch_matches("specify/foo", "specify/bar"));
        // Both sides must be specify/-shaped.
        assert!(!pr_branch_matches("feature/x", "feature/x"));
        assert!(!pr_branch_matches("specify/foo", "feature/foo"));
    }

    // ---- classify_checks --------------------------------------------------

    #[test]
    fn classify_checks_empty_list_passes() {
        assert_eq!(classify_checks(&[]), CheckOverall::Passing);
    }

    #[test]
    fn classify_checks_all_pass() {
        let checks = vec![check("ci", CheckBucket::Pass), check("lint", CheckBucket::Pass)];
        assert_eq!(classify_checks(&checks), CheckOverall::Passing);
    }

    #[test]
    fn classify_checks_skipping_does_not_block() {
        let checks = vec![check("flaky", CheckBucket::Skipping), check("ci", CheckBucket::Pass)];
        assert_eq!(classify_checks(&checks), CheckOverall::Passing);
    }

    #[test]
    fn classify_checks_pending_blocks() {
        let checks = vec![check("ci", CheckBucket::Pass), check("e2e", CheckBucket::Pending)];
        assert_eq!(classify_checks(&checks), CheckOverall::Pending);
    }

    #[test]
    fn classify_checks_failure_dominates_pending() {
        let checks = vec![check("ci", CheckBucket::Fail), check("e2e", CheckBucket::Pending)];
        assert_eq!(classify_checks(&checks), CheckOverall::Failing);
    }

    #[test]
    fn classify_checks_cancel_treated_as_failure() {
        let checks = vec![check("ci", CheckBucket::Pass), check("e2e", CheckBucket::Cancel)];
        assert_eq!(classify_checks(&checks), CheckOverall::Failing);
    }

    fn check(name: &str, bucket: CheckBucket) -> PrCheck {
        PrCheck {
            bucket,
            name: name.to_string(),
        }
    }

    // ---- classify_status (per-project state machine) ---------------------

    #[test]
    fn classify_status_no_pr_is_no_branch() {
        assert_eq!(classify_status(None, "specify/foo", None, false), MergeStatus::NoBranch);
        assert_eq!(classify_status(None, "specify/foo", None, true), MergeStatus::NoBranch);
    }

    #[test]
    fn classify_status_branch_mismatch() {
        let pr = pr_view("feature/x", PrState::Open, false);
        assert_eq!(
            classify_status(Some(&pr), "specify/foo", Some(&[]), false),
            MergeStatus::BranchPatternMismatch,
        );
    }

    #[test]
    fn classify_status_already_merged() {
        let pr = pr_view("specify/foo", PrState::Merged, true);
        assert_eq!(classify_status(Some(&pr), "specify/foo", None, false), MergeStatus::Merged,);
    }

    #[test]
    fn classify_status_closed_without_merge() {
        let pr = pr_view("specify/foo", PrState::Closed, false);
        assert_eq!(classify_status(Some(&pr), "specify/foo", None, false), MergeStatus::Closed);
    }

    #[test]
    fn classify_status_open_failing_checks() {
        let pr = pr_view("specify/foo", PrState::Open, false);
        let checks = vec![check("ci", CheckBucket::Fail)];
        assert_eq!(
            classify_status(Some(&pr), "specify/foo", Some(&checks), false),
            MergeStatus::FailedChecks,
        );
    }

    #[test]
    fn classify_status_open_pending_checks() {
        let pr = pr_view("specify/foo", PrState::Open, false);
        let checks = vec![check("ci", CheckBucket::Pending)];
        assert_eq!(
            classify_status(Some(&pr), "specify/foo", Some(&checks), false),
            MergeStatus::PendingChecks,
        );
    }

    #[test]
    fn classify_status_open_dry_run_is_would_merge() {
        let pr = pr_view("specify/foo", PrState::Open, false);
        let checks = vec![check("ci", CheckBucket::Pass)];
        assert_eq!(
            classify_status(Some(&pr), "specify/foo", Some(&checks), true),
            MergeStatus::WouldMerge,
        );
    }

    #[test]
    fn classify_status_open_passing_non_dry_run_is_merged() {
        let pr = pr_view("specify/foo", PrState::Open, false);
        let checks = vec![check("ci", CheckBucket::Pass)];
        assert_eq!(
            classify_status(Some(&pr), "specify/foo", Some(&checks), false),
            MergeStatus::Merged,
        );
    }

    fn pr_view(branch: &str, state: PrState, merged: bool) -> PrView {
        PrView {
            state,
            merged,
            head_ref_name: branch.to_string(),
            number: 42,
            url: "https://github.com/org/repo/pull/42".to_string(),
        }
    }

    // ---- mock GhClient + run_workspace_merge_impl integration ------------

    /// Recorded gh call shape — used by the mock to assert call counts
    /// (e.g. dry-run must never invoke `pr_merge_squash`).
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum GhCall {
        ViewForBranch { branch: String },
        Checks { pr: u64 },
        MergeSquash { pr: u64 },
    }

    /// Programmable `GhClient` for tests. Each project gets its own
    /// canned response keyed by branch — that's enough to exercise
    /// every classifier path through `merge_single_project`.
    struct MockGh {
        view: std::collections::HashMap<String, Result<Option<PrView>, String>>,
        checks: std::collections::HashMap<u64, Result<Vec<PrCheck>, String>>,
        merge: std::collections::HashMap<u64, Result<(), String>>,
        calls: RefCell<Vec<GhCall>>,
    }

    impl MockGh {
        fn new() -> Self {
            Self {
                view: std::collections::HashMap::new(),
                checks: std::collections::HashMap::new(),
                merge: std::collections::HashMap::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with_view(mut self, branch: &str, view: Result<Option<PrView>, String>) -> Self {
            self.view.insert(branch.to_string(), view);
            self
        }

        fn with_checks(mut self, pr: u64, checks: Result<Vec<PrCheck>, String>) -> Self {
            self.checks.insert(pr, checks);
            self
        }

        fn with_merge(mut self, pr: u64, result: Result<(), String>) -> Self {
            self.merge.insert(pr, result);
            self
        }
    }

    impl GhClient for MockGh {
        fn pr_view_for_branch(
            &self, _project_path: &Path, branch: &str,
        ) -> Result<Option<PrView>, String> {
            self.calls.borrow_mut().push(GhCall::ViewForBranch {
                branch: branch.to_string(),
            });
            self.view.get(branch).cloned().unwrap_or(Ok(None))
        }

        fn pr_checks(&self, _project_path: &Path, pr_number: u64) -> Result<Vec<PrCheck>, String> {
            self.calls.borrow_mut().push(GhCall::Checks { pr: pr_number });
            self.checks.get(&pr_number).cloned().unwrap_or(Ok(Vec::new()))
        }

        fn pr_merge_squash(&self, _project_path: &Path, pr_number: u64) -> Result<(), String> {
            self.calls.borrow_mut().push(GhCall::MergeSquash { pr: pr_number });
            self.merge.get(&pr_number).cloned().unwrap_or(Ok(()))
        }
    }

    /// Build a registry with the named projects (URL: `git@github.com:org/<n>.git`,
    /// schema: `omnia@v1`) — covers the common shape; symlink-mode tests
    /// can construct registries by hand.
    fn registry_with(names: &[&str]) -> Registry {
        Registry {
            version: 1,
            projects: names
                .iter()
                .map(|n| RegistryProject {
                    name: (*n).to_string(),
                    url: format!("git@github.com:org/{n}.git"),
                    schema: "omnia@v1".to_string(),
                    description: Some(format!("{n} service")),
                    contracts: None,
                })
                .collect(),
        }
    }

    #[test]
    fn empty_registry_errors_before_any_gh_call() {
        let registry = Registry {
            version: 1,
            projects: vec![],
        };
        let mock = MockGh::new();
        let err = run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
            .expect_err("empty registry must error");
        assert!(matches!(err, Error::Config(_)), "expected Error::Config, got {err:?}");
        assert!(mock.calls.borrow().is_empty(), "no gh calls should fire on empty registry");
    }

    #[test]
    fn no_branch_when_pr_view_returns_none() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new().with_view("specify/foo", Ok(None));
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, MergeStatus::NoBranch);
        assert_eq!(results[0].name, "alpha");
        assert!(
            results[0].detail.as_deref().is_some_and(|d| d.contains("specify/foo")),
            "no-branch detail must surface the expected branch, got: {:?}",
            results[0].detail,
        );
    }

    #[test]
    fn branch_pattern_mismatch_rejects_off_pattern_pr() {
        let registry = registry_with(&["alpha"]);
        let bogus = PrView {
            state: PrState::Open,
            merged: false,
            head_ref_name: "feature/foo".to_string(),
            number: 1,
            url: "https://github.com/org/alpha/pull/1".to_string(),
        };
        let mock = MockGh::new().with_view("specify/foo", Ok(Some(bogus)));
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::BranchPatternMismatch);
        // Diagnostic surfaces the literal expected branch (RFC-9 §4A
        // "Surface the literal string `specify/<initiative-name>`").
        assert!(
            results[0].detail.as_deref().is_some_and(|d| d.contains("specify/foo")),
            "diagnostic must include expected branch, got: {:?}",
            results[0].detail,
        );
        // Never invokes merge.
        assert!(
            !mock.calls.borrow().iter().any(|c| matches!(c, GhCall::MergeSquash { .. })),
            "branch-pattern-mismatch must not call merge",
        );
    }

    #[test]
    fn already_merged_short_circuits() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "https://github.com/org/alpha/pull/7".to_string(),
            })),
        );
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::Merged);
        assert_eq!(results[0].pr_number, Some(7));
        // Already-merged path must not invoke checks or merge.
        let calls = mock.calls.borrow();
        assert!(!calls.iter().any(|c| matches!(c, GhCall::Checks { .. })));
        assert!(!calls.iter().any(|c| matches!(c, GhCall::MergeSquash { .. })));
    }

    #[test]
    fn closed_without_merge_classifies_closed() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Closed,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 9,
                url: "u".to_string(),
            })),
        );
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::Closed);
    }

    #[test]
    fn pending_checks_reported_without_merging() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Open,
                    merged: false,
                    head_ref_name: "specify/foo".to_string(),
                    number: 11,
                    url: "u".to_string(),
                })),
            )
            .with_checks(
                11,
                Ok(vec![
                    PrCheck {
                        bucket: CheckBucket::Pass,
                        name: "lint".to_string(),
                    },
                    PrCheck {
                        bucket: CheckBucket::Pending,
                        name: "e2e".to_string(),
                    },
                ]),
            );
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::PendingChecks);
        assert!(
            results[0].detail.as_deref().is_some_and(|d| d.contains("e2e")),
            "pending-checks detail must name the workflow, got: {:?}",
            results[0].detail,
        );
        assert!(
            !mock.calls.borrow().iter().any(|c| matches!(c, GhCall::MergeSquash { .. })),
            "pending checks must not invoke merge",
        );
    }

    #[test]
    fn failed_checks_reported_without_merging() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Open,
                    merged: false,
                    head_ref_name: "specify/foo".to_string(),
                    number: 13,
                    url: "u".to_string(),
                })),
            )
            .with_checks(
                13,
                Ok(vec![PrCheck {
                    bucket: CheckBucket::Fail,
                    name: "ci".to_string(),
                }]),
            );
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::FailedChecks);
        assert!(
            results[0].detail.as_deref().is_some_and(|d| d.contains("ci")),
            "failed-checks detail must name the workflow, got: {:?}",
            results[0].detail,
        );
        assert!(
            !mock.calls.borrow().iter().any(|c| matches!(c, GhCall::MergeSquash { .. })),
            "failed checks must not invoke merge",
        );
    }

    #[test]
    fn green_pr_merged_in_real_run() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Open,
                    merged: false,
                    head_ref_name: "specify/foo".to_string(),
                    number: 15,
                    url: "u".to_string(),
                })),
            )
            .with_checks(
                15,
                Ok(vec![PrCheck {
                    bucket: CheckBucket::Pass,
                    name: "ci".to_string(),
                }]),
            )
            .with_merge(15, Ok(()));
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::Merged);
        assert_eq!(results[0].pr_number, Some(15));
        assert!(
            mock.calls.borrow().iter().any(|c| matches!(c, GhCall::MergeSquash { pr: 15 })),
            "real run must invoke merge",
        );
    }

    #[test]
    fn green_pr_dry_run_does_not_invoke_merge() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Open,
                    merged: false,
                    head_ref_name: "specify/foo".to_string(),
                    number: 17,
                    url: "u".to_string(),
                })),
            )
            .with_checks(
                17,
                Ok(vec![PrCheck {
                    bucket: CheckBucket::Pass,
                    name: "ci".to_string(),
                }]),
            )
            .with_merge(17, Err("must not be called".to_string()));
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], true)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::WouldMerge);
        let calls = mock.calls.borrow();
        let merge_calls = calls.iter().filter(|c| matches!(c, GhCall::MergeSquash { .. })).count();
        assert_eq!(merge_calls, 0, "dry-run must never invoke gh pr merge, calls: {calls:?}");
    }

    #[test]
    fn merge_failure_reports_failed_status_with_detail() {
        let registry = registry_with(&["alpha"]);
        let mock = MockGh::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Open,
                    merged: false,
                    head_ref_name: "specify/foo".to_string(),
                    number: 19,
                    url: "u".to_string(),
                })),
            )
            .with_checks(
                19,
                Ok(vec![PrCheck {
                    bucket: CheckBucket::Pass,
                    name: "ci".to_string(),
                }]),
            )
            .with_merge(19, Err("merge conflict".to_string()));
        let results =
            run_workspace_merge_impl(Path::new("/proj"), "foo", &registry, &mock, &[], false)
                .expect("ok");
        assert_eq!(results[0].status, MergeStatus::Failed);
        assert!(
            results[0].detail.as_deref().is_some_and(|d| d.contains("merge conflict")),
            "failed detail must surface gh stderr, got: {:?}",
            results[0].detail,
        );
    }

    /// Wrapping mock used by `batch_continues_after_one_project_failure`
    /// to simulate a per-project gh failure. The mock-by-branch alone
    /// cannot express "fail for project X but pass for project Y" since
    /// every project is queried with the same branch, so we layer a
    /// path-keyed override on top.
    struct PerProjectMock {
        base: MockGh,
        fail_for_path: PathBuf,
    }

    impl GhClient for PerProjectMock {
        fn pr_view_for_branch(
            &self, project_path: &Path, branch: &str,
        ) -> Result<Option<PrView>, String> {
            if project_path == self.fail_for_path {
                return Err("simulated gh failure".to_string());
            }
            self.base.pr_view_for_branch(project_path, branch)
        }

        fn pr_checks(&self, project_path: &Path, pr_number: u64) -> Result<Vec<PrCheck>, String> {
            self.base.pr_checks(project_path, pr_number)
        }

        fn pr_merge_squash(&self, project_path: &Path, pr_number: u64) -> Result<(), String> {
            self.base.pr_merge_squash(project_path, pr_number)
        }
    }

    /// One project failing must not abort processing of the others —
    /// per the brief: "Failure on one project must not abort
    /// processing of others."
    #[test]
    fn batch_continues_after_one_project_failure() {
        let registry = registry_with(&["alpha", "beta", "gamma"]);
        // Base mock applies to alpha + beta + gamma; the per-project
        // wrapper short-circuits beta with a simulated gh error.
        let mock = MockGh::new().with_view("specify/foo", Ok(None));
        let project_dir = Path::new("/proj");
        let workspace_base = workspace_base(project_dir);
        let beta_path = workspace_base.join("beta");
        let per_proj = PerProjectMock {
            base: mock,
            fail_for_path: beta_path,
        };

        let results =
            run_workspace_merge_impl(project_dir, "foo", &registry, &per_proj, &[], false)
                .expect("ok");
        assert_eq!(results.len(), 3);
        // alpha + gamma classify as no-branch (the base mock returns None).
        assert_eq!(results[0].name, "alpha");
        assert_eq!(results[0].status, MergeStatus::NoBranch);
        assert_eq!(results[1].name, "beta");
        assert_eq!(results[1].status, MergeStatus::Failed);
        assert!(
            results[1].detail.as_deref().is_some_and(|d| d.contains("simulated gh failure")),
            "beta failure must carry detail, got: {:?}",
            results[1].detail,
        );
        assert_eq!(results[2].name, "gamma");
        assert_eq!(results[2].status, MergeStatus::NoBranch);
    }

    #[test]
    fn project_filter_restricts_to_named_projects() {
        let registry = registry_with(&["alpha", "beta", "gamma"]);
        let mock = MockGh::new().with_view("specify/foo", Ok(None));
        let results = run_workspace_merge_impl(
            Path::new("/proj"),
            "foo",
            &registry,
            &mock,
            &["beta".to_string()],
            false,
        )
        .expect("ok");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "beta");
    }
}
