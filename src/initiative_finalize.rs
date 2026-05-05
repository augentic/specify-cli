//! `specify initiative finalize` — initiative landing closure (RFC-9 §4C).
//!
//! Closure verb for the platform-first loop. `workspace push` ships the
//! commits; `workspace merge` (RFC-9 §4A) lands the PRs; `plan archive`
//! sweeps local plan state. `initiative finalize` is the verb that
//! confirms the **whole** initiative is landed (every per-project PR
//! merged on remote, every workspace clone clean) and atomically sweeps
//! `plan.yaml`, `initiative.md`, and `.specify/plans/<name>/` into
//! `.specify/archive/plans/<YYYYMMDD>-<name>/`. With `--clean`, prunes
//! `.specify/workspace/<peer>/` clones after the archive completes.
//!
//! ## Defence in depth
//!
//! Three guards layer up before the archive runs. **Any guard failure
//! is fatal** — finalize is all-or-nothing. The on-disk state is
//! identical before and after a refused finalize.
//!
//! 1. **Plan-presence + terminal-state guard.** `.specify/plan.yaml`
//!    must exist and every entry must be in a terminal state for
//!    finalize purposes — `done`, `failed`, or `skipped` (the in-Plan
//!    equivalent of the brief's `dropped`). Anything pending,
//!    in-progress, or blocked refuses with `non-terminal-entries-present`.
//! 2. **Per-project PR-state guard.** For each registry project, query
//!    `gh pr view --json state,merged,headRefName,number,url` against
//!    the project's workspace clone. Statuses: `merged`, `unmerged`,
//!    `closed`, `no-branch`, `branch-pattern-mismatch`, `failed`. Only
//!    `merged` and `no-branch` pass.
//! 3. **Workspace-cleanliness guard.** For each workspace clone,
//!    `git status --porcelain` must be empty. Dirty clones surface as
//!    status `dirty` and refuse — protecting the operator from losing
//!    uncommitted work to an inadvertent `--clean`.
//!
//! ## Composition
//!
//! `initiative finalize` is independent of `workspace merge`. The two
//! valid operator paths are:
//!
//! - **Autonomous:** `workspace merge` (RFC-9 §4A) merges every PR with
//!   green CI; `initiative finalize` confirms the merges and archives.
//! - **Supervised:** the operator merges PRs by hand on the forge;
//!   `initiative finalize` confirms and archives.
//!
//! ## Atomicity
//!
//! `Plan::archive` preflights both destinations (`<name>-<date>.yaml`
//! and `<name>-<date>/`) before any move, so a collision returns an
//! error before any file is touched. `--clean` runs **after** the
//! archive, so a failed archive leaves clones intact. `--dry-run`
//! never invokes archive or clean and never spawns `gh pr merge`.

#![allow(clippy::needless_pass_by_value)]

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use specify_change::Plan;
use specify_change::plan::Status;
use specify_error::Error;
use specify_schema::{Registry, RegistryProject};

use crate::config::ProjectConfig;
use crate::workspace_merge::{
    GhClient, PrState, PrView, RealGhClient, SPECIFY_BRANCH_PREFIX, pr_branch_matches,
    project_path_for,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-project classification for `specify initiative finalize`.
///
/// Display strings are kebab-case and match the JSON `status` value.
/// Skill authors and operators rely on this vocabulary; treat it as a
/// stable wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeStatus {
    /// PR is `MERGED` on remote — passing.
    Merged,
    /// PR exists, branch matches, but has not landed (state `OPEN`).
    /// Refuses finalize.
    Unmerged,
    /// PR was `CLOSED` without merging. Refuses finalize.
    Closed,
    /// No PR on `specify/<initiative-name>` for this project — passing
    /// (e.g. the project was assigned no work in this initiative, or
    /// the operator merged via the GitHub web UI and deleted the
    /// branch).
    NoBranch,
    /// A PR exists but its `headRefName` is not the expected branch.
    /// Defence in depth — branch-pattern guard from RFC-9 §4A applies
    /// here too.
    BranchPatternMismatch,
    /// `git status --porcelain` for the workspace clone is non-empty.
    /// Refuses finalize even without `--clean`, to protect uncommitted
    /// work from a subsequent `--clean` run.
    Dirty,
    /// Generic shell-out failure (gh missing, unparseable JSON, network
    /// error, …). Refuses finalize.
    Failed,
}

impl FinalizeStatus {
    /// Stable kebab-case identifier — the JSON wire value and the
    /// human-readable status column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Unmerged => "unmerged",
            Self::Closed => "closed",
            Self::NoBranch => "no-branch",
            Self::BranchPatternMismatch => "branch-pattern-mismatch",
            Self::Dirty => "dirty",
            Self::Failed => "failed",
        }
    }

    /// Whether this per-project status counts as a passing classification
    /// for finalize purposes. Only `merged` and `no-branch` pass.
    #[must_use]
    pub const fn is_passing(self) -> bool {
        matches!(self, Self::Merged | Self::NoBranch)
    }
}

impl std::fmt::Display for FinalizeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-project result row, surfaced in both text and JSON output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct FinalizeProjectResult {
    /// Registry project name.
    pub name: String,
    /// Outcome of the finalize attempt.
    #[serde(serialize_with = "serialize_status")]
    pub status: FinalizeStatus,
    /// PR number when discovered (any state).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    /// PR URL when discovered (any state).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `headRefName` reported by `gh pr view`. Surfaced in
    /// diagnostics for `branch-pattern-mismatch`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_ref_name: Option<String>,
    /// `true` when `git status --porcelain` was non-empty. Independent
    /// of the PR-state status — surfaced even when status is `merged`
    /// but the local clone is dirty so the operator sees both signals
    /// (though the row's overall status will then be `dirty`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    /// Free-form context — gh stderr, parse errors, hint at remediation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn serialize_status<S: serde::Serializer>(
    status: &FinalizeStatus, s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(status.as_str())
}

/// Per-status counters for the summary row.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FinalizeSummaryCounts {
    /// PRs in `MERGED` state on remote.
    pub merged: usize,
    /// PRs in `OPEN` state — refuses finalize.
    pub unmerged: usize,
    /// PRs in `CLOSED` state without merge — refuses finalize.
    pub closed: usize,
    /// Projects without a `specify/<initiative-name>` PR — passes.
    pub no_branch: usize,
    /// PRs whose `headRefName` did not match — refuses finalize.
    pub branch_pattern_mismatch: usize,
    /// Workspace clones with a non-empty `git status --porcelain`.
    pub dirty: usize,
    /// Generic shell-out / network failures.
    pub failed: usize,
}

/// Top-level outcome of a finalize run.
///
/// Serialised as the JSON envelope payload. `finalized` is the
/// authoritative wire flag for "did the archive land?" — `false` when
/// any guard refused, `true` when the archive succeeded (real run) or
/// when the dry-run preview classified everything as ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct FinalizeOutcome {
    /// Initiative name (= `plan.yaml:name`).
    pub initiative: String,
    /// `true` when the archive landed on a real run, or when a dry-run
    /// preview classified the initiative as ready to finalize.
    pub finalized: bool,
    /// `specify/<initiative-name>` — surfaced for skill authors that
    /// echo the literal branch in operator-facing output.
    pub expected_branch: String,
    /// Per-project rows, one per registry entry.
    pub projects: Vec<FinalizeProjectResult>,
    /// Aggregate counts — same vocabulary as the per-project rows.
    pub summary: FinalizeSummaryCounts,
    /// Path of the archived `plan.yaml` (e.g.
    /// `.specify/archive/plans/foo-20260428.yaml`). `None` on dry-run
    /// or refused finalize.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<String>,
    /// Path of the archived `<name>-<date>/` directory when the plans
    /// working dir or `initiative.md` was co-moved. `None` when neither
    /// existed (or on dry-run / refused finalize).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_plans_dir: Option<String>,
    /// Names of projects whose `.specify/workspace/<name>/` clone was
    /// pruned by `--clean`. Empty when `--clean` was absent or when the
    /// archive was refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cleaned: Vec<String>,
    /// Echo of the `--dry-run` flag. `Some(true)` only when the run
    /// was a dry-run; serialised omitted otherwise so real-run output
    /// stays minimal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

// ---------------------------------------------------------------------------
// IO trait — abstraction over `gh` + `git status` (testable seam)
// ---------------------------------------------------------------------------

/// Abstraction over the external probes finalize depends on.
///
/// Two methods, one per guard's external side-effect: `pr_view_for_branch`
/// for the GitHub PR-state guard (delegates to `gh pr view`) and
/// `is_dirty` for the workspace-cleanliness guard (delegates to
/// `git status --porcelain`).
///
/// The CLI binary plugs in [`RealFinalizeProbe`]; tests substitute a
/// mock that records calls and replays canned outputs. Both methods
/// operate **inside** `project_path` (i.e. the workspace clone, or
/// the source path for symlink-mode projects).
pub trait FinalizeProbe {
    /// Look up the open/closed/merged PR for `branch`. Returns
    /// `Ok(None)` when no PR exists on that branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
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

/// Default [`FinalizeProbe`] backed by `gh` + `git`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFinalizeProbe;

impl FinalizeProbe for RealFinalizeProbe {
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

// ---------------------------------------------------------------------------
// Pure logic — terminal-state, PR classification, status combine
// ---------------------------------------------------------------------------

/// Whether a plan-entry status counts as terminal for finalize.
///
/// Per the brief, `done` / `failed` / `dropped` are terminal; the
/// in-`Plan` representation maps `dropped` to [`Status::Skipped`]
/// (the latter is what `specify change drop` surfaces back to the
/// plan).
#[must_use]
pub const fn is_terminal_for_finalize(status: Status) -> bool {
    matches!(status, Status::Done | Status::Failed | Status::Skipped)
}

/// Classify a single project's PR state without performing any
/// mutation.
///
/// Pure: takes the gh observations and the expected branch, returns a
/// [`FinalizeStatus`] from the PR-state half of the universe. The
/// caller layers in dirtiness via [`combine_status`].
///
/// 1. No PR ⇒ `no-branch`.
/// 2. `headRefName` mismatch ⇒ `branch-pattern-mismatch`.
/// 3. Already merged ⇒ `merged`.
/// 4. Closed without merge ⇒ `closed`.
/// 5. Open ⇒ `unmerged`.
#[must_use]
pub fn classify_pr_state(pr: Option<&PrView>, expected_branch: &str) -> FinalizeStatus {
    let Some(pr) = pr else {
        return FinalizeStatus::NoBranch;
    };
    if !pr_branch_matches(&pr.head_ref_name, expected_branch) {
        return FinalizeStatus::BranchPatternMismatch;
    }
    if pr.merged || matches!(pr.state, PrState::Merged) {
        return FinalizeStatus::Merged;
    }
    if matches!(pr.state, PrState::Closed) {
        return FinalizeStatus::Closed;
    }
    FinalizeStatus::Unmerged
}

/// Combine the PR-state classification with the dirty-clone observation.
///
/// `dirty` overrides any non-failure PR state — a dirty clone is fatal
/// regardless of the PR's status, because a subsequent `--clean` would
/// drop the uncommitted work. `Failed` (gh shell error) takes precedence
/// over `Dirty` so the operator can see why the probe broke.
#[must_use]
pub const fn combine_status(pr_status: FinalizeStatus, dirty: bool) -> FinalizeStatus {
    if matches!(pr_status, FinalizeStatus::Failed) {
        return FinalizeStatus::Failed;
    }
    if dirty {
        return FinalizeStatus::Dirty;
    }
    pr_status
}

/// Walk the plan and return the names of entries whose status is not a
/// terminal-for-finalize state. List order matches plan order so the
/// diagnostic is stable.
#[must_use]
pub fn non_terminal_entries(plan: &Plan) -> Vec<String> {
    plan.changes
        .iter()
        .filter(|c| !is_terminal_for_finalize(c.status))
        .map(|c| c.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Orchestration — generic over FinalizeProbe for testability
// ---------------------------------------------------------------------------

/// Inputs that don't fit the per-project loop.
pub struct FinalizeInputs<'a> {
    /// Project root directory (`.specify/` lives directly under here).
    pub project_dir: &'a Path,
    /// Loaded plan — owns the canonical initiative name.
    pub plan: &'a Plan,
    /// Loaded registry — owns the project list.
    pub registry: &'a Registry,
    /// `--clean` flag.
    pub clean: bool,
    /// `--dry-run` flag.
    pub dry_run: bool,
}

/// Top-level error sentinel for finalize.
///
/// Distinct from per-project failures: these are the **whole-run**
/// refusals (plan absent, non-terminal entries) that surface as a
/// hard error from the CLI handler with their own diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizeError {
    /// `.specify/plan.yaml` does not exist. Recovery: this is the
    /// signal that the initiative is already finalized.
    PlanNotFound,
    /// One or more plan entries are not in a terminal state. Carries
    /// the offending entry names in plan order.
    NonTerminalEntries(Vec<String>),
}

/// Run the whole finalize pipeline.
///
/// Order:
/// 1. Plan-presence guard (caller's responsibility — call
///    [`load_plan_or_refuse`] first; the `Plan` arrives here loaded).
/// 2. Plan terminal-state guard (returns
///    [`FinalizeError::NonTerminalEntries`] when not satisfied).
/// 3. Per-project probes — PR state + dirty clone.
/// 4. When all projects pass and not `--dry-run`: archive plan + clean.
/// 5. Always returns a [`FinalizeOutcome`] for consumers; a refused
///    finalize has `finalized: false` and pinpoints the failing
///    projects.
///
/// # Errors
///
/// Returns [`FinalizeError`] for whole-run refusals. Per-project
/// failures live in [`FinalizeOutcome::projects`] and never bubble up.
pub fn run_finalize<P: FinalizeProbe>(
    inputs: FinalizeInputs<'_>, probe: &P,
) -> Result<FinalizeOutcome, FinalizeError> {
    // Guard: terminal states.
    let outstanding = non_terminal_entries(inputs.plan);
    if !outstanding.is_empty() {
        return Err(FinalizeError::NonTerminalEntries(outstanding));
    }

    let initiative_name = inputs.plan.name.clone();
    let expected_branch = format!("{SPECIFY_BRANCH_PREFIX}{initiative_name}");
    let workspace_base = ProjectConfig::specify_dir(inputs.project_dir).join("workspace");

    // Guard: per-project PR state + dirty clones.
    let mut projects: Vec<FinalizeProjectResult> =
        Vec::with_capacity(inputs.registry.projects.len());
    for rp in &inputs.registry.projects {
        let path = project_path_for(inputs.project_dir, &workspace_base, rp);
        projects.push(probe_single_project(probe, &path, rp, &expected_branch, inputs.clean));
    }

    let summary = summarise(&projects);
    let any_refusing = projects.iter().any(|p| !p.status.is_passing());

    let mut outcome = FinalizeOutcome {
        initiative: initiative_name,
        finalized: false,
        expected_branch,
        projects,
        summary,
        archived: None,
        archived_plans_dir: None,
        cleaned: Vec::new(),
        dry_run: inputs.dry_run.then_some(true),
    };

    if any_refusing {
        return Ok(outcome);
    }

    // Dry-run: preview only. Don't archive, don't clean.
    if inputs.dry_run {
        outcome.finalized = true;
        return Ok(outcome);
    }

    // All guards passed — archive (atomic) and optionally clean.
    let plan_path = ProjectConfig::plan_path(inputs.project_dir);
    let initiative_path = ProjectConfig::initiative_path(inputs.project_dir);
    let archive_dir = ProjectConfig::archive_dir(inputs.project_dir).join("plans");
    match Plan::archive(&plan_path, &initiative_path, &archive_dir, /* force = */ true) {
        Ok((archived, archived_plans_dir)) => {
            outcome.archived = Some(archived.display().to_string());
            outcome.archived_plans_dir =
                archived_plans_dir.as_ref().map(|p| p.display().to_string());
        }
        Err(err) => {
            // Atomicity: Plan::archive preflights both destinations
            // before any move, so a failure here leaves the on-disk
            // state untouched. Surface the underlying error in `detail`
            // and return finalized=false.
            outcome.archived = None;
            outcome.archived_plans_dir = None;
            // Use a generic "archive-failed" status on every project so
            // the operator sees a clear archive-time failure marker.
            // We do NOT mutate per-project rows — the actual cause is
            // an io / config error from Plan::archive; bubble it as a
            // human-readable detail on a synthetic project entry would
            // muddy the wire format. Instead, we surface the error via
            // a sentinel on the first project's detail — keeping the
            // finalized flag false is the load-bearing signal for
            // skills.
            outcome.finalized = false;
            // Best-effort: stamp the error onto a `detail` field on
            // the first project row (or append a synthetic one if the
            // registry is empty).
            let detail = format!("plan archive failed: {err}");
            if let Some(first) = outcome.projects.first_mut() {
                first.detail = Some(detail);
            } else {
                outcome.projects.push(FinalizeProjectResult {
                    name: "<archive>".to_string(),
                    status: FinalizeStatus::Failed,
                    pr_number: None,
                    url: None,
                    head_ref_name: None,
                    dirty: None,
                    detail: Some(detail),
                });
            }
            return Ok(outcome);
        }
    }

    if inputs.clean {
        outcome.cleaned = clean_workspace_clones(&workspace_base, inputs.registry);
    }

    outcome.finalized = true;
    Ok(outcome)
}

/// Probe a single project — combine PR state and dirty observation
/// into one [`FinalizeProjectResult`] row.
fn probe_single_project<P: FinalizeProbe>(
    probe: &P, project_path: &Path, rp: &RegistryProject, expected_branch: &str, clean: bool,
) -> FinalizeProjectResult {
    let name = &rp.name;
    let pr_view = probe.pr_view_for_branch(project_path, expected_branch);

    let (pr_status, pr_number, url, head_ref_name, pr_detail) = match pr_view {
        Ok(None) => (FinalizeStatus::NoBranch, None, None, None, None::<String>),
        Ok(Some(pr)) => {
            let status = classify_pr_state(Some(&pr), expected_branch);
            let detail = match status {
                FinalizeStatus::BranchPatternMismatch => Some(format!(
                    "PR #{} headRefName `{}` does not match expected branch `{}`; refusing to finalize",
                    pr.number, pr.head_ref_name, expected_branch
                )),
                FinalizeStatus::Closed => {
                    Some(format!("PR #{} is CLOSED without merge", pr.number))
                }
                FinalizeStatus::Unmerged => Some(format!(
                    "PR #{} is still OPEN; merge it (or run `specify workspace merge`) before finalizing",
                    pr.number
                )),
                _ => None,
            };
            (status, Some(pr.number), Some(pr.url), Some(pr.head_ref_name), detail)
        }
        Err(err) => (FinalizeStatus::Failed, None, None, None, Some(err)),
    };

    // Only check porcelain when the path actually exists — `gh` will
    // already have failed (and produced `Failed`) for a missing clone.
    let dirty = project_path.exists() && probe.is_dirty(project_path);

    let final_status = combine_status(pr_status, dirty);

    let detail = match final_status {
        FinalizeStatus::Dirty => Some(if clean {
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

    FinalizeProjectResult {
        name: name.clone(),
        status: final_status,
        pr_number,
        url,
        head_ref_name,
        dirty: Some(dirty),
        detail,
    }
}

/// Aggregate per-status counts for the summary row.
#[must_use]
pub fn summarise(results: &[FinalizeProjectResult]) -> FinalizeSummaryCounts {
    let mut s = FinalizeSummaryCounts::default();
    for r in results {
        match r.status {
            FinalizeStatus::Merged => s.merged += 1,
            FinalizeStatus::Unmerged => s.unmerged += 1,
            FinalizeStatus::Closed => s.closed += 1,
            FinalizeStatus::NoBranch => s.no_branch += 1,
            FinalizeStatus::BranchPatternMismatch => s.branch_pattern_mismatch += 1,
            FinalizeStatus::Dirty => s.dirty += 1,
            FinalizeStatus::Failed => s.failed += 1,
        }
    }
    s
}

/// Remove `.specify/workspace/<name>/` clones for every non-symlink
/// registry project. Best-effort: a single project's failure is
/// recorded silently (the archive has already landed; clean is the
/// optional bonus step). Returns the names of successfully-cleaned
/// projects so the caller can surface them.
fn clean_workspace_clones(workspace_base: &Path, registry: &Registry) -> Vec<String> {
    let mut cleaned = Vec::new();
    for rp in &registry.projects {
        // Symlink projects point at source repositories the operator
        // owns separately — never delete them on `--clean`.
        if rp.url_materialises_as_symlink() {
            continue;
        }
        let slot = workspace_base.join(&rp.name);
        if !slot.exists() {
            continue;
        }
        if std::fs::remove_dir_all(&slot).is_ok() {
            cleaned.push(rp.name.clone());
        }
    }
    cleaned
}

/// Plan-presence guard: load `plan.yaml` (at the repo root) or return
/// [`FinalizeError::PlanNotFound`].
///
/// # Errors
///
/// Bubbles up `Plan::load` errors verbatim — a malformed plan is a
/// real failure, not a "plan absent" sentinel.
pub fn load_plan_or_refuse(project_dir: &Path) -> Result<Result<Plan, FinalizeError>, Error> {
    let plan_path = ProjectConfig::plan_path(project_dir);
    if !plan_path.exists() {
        return Ok(Err(FinalizeError::PlanNotFound));
    }
    Ok(Ok(Plan::load(&plan_path)?))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use std::path::PathBuf;

    use specify_change::plan::Entry;
    use specify_schema::RegistryProject;
    use tempfile::TempDir;

    use super::*;

    // ---- pure helpers -----------------------------------------------------

    #[test]
    fn terminal_states_accept_done_failed_skipped() {
        assert!(is_terminal_for_finalize(Status::Done));
        assert!(is_terminal_for_finalize(Status::Failed));
        assert!(is_terminal_for_finalize(Status::Skipped));
    }

    #[test]
    fn terminal_states_reject_pending_in_progress_blocked() {
        assert!(!is_terminal_for_finalize(Status::Pending));
        assert!(!is_terminal_for_finalize(Status::InProgress));
        assert!(!is_terminal_for_finalize(Status::Blocked));
    }

    #[test]
    fn classify_pr_state_no_pr_is_no_branch() {
        assert_eq!(classify_pr_state(None, "specify/foo"), FinalizeStatus::NoBranch);
    }

    #[test]
    fn classify_pr_state_branch_mismatch() {
        let pr = pr_view("feature/x", PrState::Open, false);
        assert_eq!(
            classify_pr_state(Some(&pr), "specify/foo"),
            FinalizeStatus::BranchPatternMismatch,
        );
    }

    #[test]
    fn classify_pr_state_merged_short_circuits() {
        let pr = pr_view("specify/foo", PrState::Merged, true);
        assert_eq!(classify_pr_state(Some(&pr), "specify/foo"), FinalizeStatus::Merged);
    }

    #[test]
    fn classify_pr_state_closed_without_merge() {
        let pr = pr_view("specify/foo", PrState::Closed, false);
        assert_eq!(classify_pr_state(Some(&pr), "specify/foo"), FinalizeStatus::Closed);
    }

    #[test]
    fn classify_pr_state_open_is_unmerged() {
        let pr = pr_view("specify/foo", PrState::Open, false);
        assert_eq!(classify_pr_state(Some(&pr), "specify/foo"), FinalizeStatus::Unmerged);
    }

    #[test]
    fn combine_status_dirty_overrides_passing() {
        assert_eq!(combine_status(FinalizeStatus::Merged, true), FinalizeStatus::Dirty,);
        assert_eq!(combine_status(FinalizeStatus::NoBranch, true), FinalizeStatus::Dirty,);
    }

    #[test]
    fn combine_status_failed_takes_precedence_over_dirty() {
        assert_eq!(combine_status(FinalizeStatus::Failed, true), FinalizeStatus::Failed,);
    }

    #[test]
    fn combine_status_clean_passes_through() {
        assert_eq!(combine_status(FinalizeStatus::Merged, false), FinalizeStatus::Merged,);
        assert_eq!(combine_status(FinalizeStatus::Unmerged, false), FinalizeStatus::Unmerged,);
    }

    #[test]
    fn non_terminal_entries_lists_in_plan_order() {
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![
                entry("a", Status::Done),
                entry("b", Status::Pending),
                entry("c", Status::InProgress),
                entry("d", Status::Done),
                entry("e", Status::Blocked),
            ],
        };
        assert_eq!(non_terminal_entries(&plan), vec!["b", "c", "e"]);
    }

    #[test]
    fn non_terminal_entries_empty_when_all_terminal() {
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![
                entry("a", Status::Done),
                entry("b", Status::Failed),
                entry("c", Status::Skipped),
            ],
        };
        assert!(non_terminal_entries(&plan).is_empty());
    }

    fn entry(name: &str, status: Status) -> Entry {
        Entry {
            name: name.to_string(),
            project: None,
            schema: Some("omnia@v1".to_string()),
            status,
            depends_on: Vec::new(),
            sources: Vec::new(),
            context: Vec::new(),
            description: None,
            status_reason: None,
        }
    }

    fn pr_view(branch: &str, state: PrState, merged: bool) -> PrView {
        PrView {
            state,
            merged,
            head_ref_name: branch.to_string(),
            number: 42,
            url: format!("https://github.com/org/repo/pull/{}", 42),
        }
    }

    // ---- mock probe -------------------------------------------------------

    /// Programmable probe — replays canned `gh pr view` results keyed
    /// by branch and dirty flags keyed by canonical project path.
    struct MockProbe {
        view: HashMap<String, Result<Option<PrView>, String>>,
        dirty: HashMap<PathBuf, bool>,
        calls: RefCell<Vec<String>>,
    }

    impl MockProbe {
        fn new() -> Self {
            Self {
                view: HashMap::new(),
                dirty: HashMap::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with_view(mut self, branch: &str, view: Result<Option<PrView>, String>) -> Self {
            self.view.insert(branch.to_string(), view);
            self
        }

        fn with_dirty(mut self, path: PathBuf, dirty: bool) -> Self {
            self.dirty.insert(path, dirty);
            self
        }
    }

    impl FinalizeProbe for MockProbe {
        fn pr_view_for_branch(
            &self, _project_path: &Path, branch: &str,
        ) -> Result<Option<PrView>, String> {
            self.calls.borrow_mut().push(format!("view:{branch}"));
            self.view.get(branch).cloned().unwrap_or(Ok(None))
        }

        fn is_dirty(&self, project_path: &Path) -> bool {
            self.calls.borrow_mut().push(format!("dirty:{}", project_path.display()));
            self.dirty.get(project_path).copied().unwrap_or(false)
        }
    }

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

    fn plan_named(name: &str) -> Plan {
        Plan {
            name: name.to_string(),
            sources: BTreeMap::new(),
            changes: Vec::new(),
        }
    }

    fn plan_with_entries(name: &str, entries: Vec<Entry>) -> Plan {
        Plan {
            name: name.to_string(),
            sources: BTreeMap::new(),
            changes: entries,
        }
    }

    // ---- guard: non-terminal entries -------------------------------------

    #[test]
    fn refuses_when_plan_has_non_terminal_entries() {
        let tmp = TempDir::new().expect("tempdir");
        let plan =
            plan_with_entries("foo", vec![entry("a", Status::Done), entry("b", Status::Pending)]);
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new();
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let err = run_finalize(inputs, &probe).expect_err("non-terminal must refuse");
        assert!(matches!(err, FinalizeError::NonTerminalEntries(ref names) if names == &["b"]));
        // Probe must not have been called — guard runs before any IO.
        assert!(probe.calls.borrow().is_empty(), "no probes on non-terminal refusal");
    }

    // ---- guard: per-project PR states ------------------------------------

    #[test]
    fn finalizes_with_no_clones_and_no_registry_passes() {
        // Edge case: plan has no entries (vacuously terminal) and the
        // registry has no projects. The archive path is still
        // exercised — finalize must succeed.
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let plan_path = tmp.path().join("plan.yaml");
        fs::write(&plan_path, "name: foo\nchanges: []\n").expect("seed plan");

        let plan = plan_named("foo");
        let registry = Registry {
            version: 1,
            projects: vec![],
        };
        let probe = MockProbe::new();
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(outcome.finalized);
        assert!(outcome.projects.is_empty());
        assert!(outcome.archived.is_some(), "archive must have run");
        assert!(!plan_path.exists(), "plan.yaml must have moved into archive");
    }

    #[test]
    fn refuses_when_one_project_pr_is_unmerged() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let plan_path = tmp.path().join("plan.yaml");
        fs::write(&plan_path, "name: foo\nchanges: []\n").expect("seed plan");

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "https://github.com/org/alpha/pull/7".to_string(),
            })),
        );
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(!outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::Unmerged);
        assert_eq!(outcome.projects[0].pr_number, Some(7));
        assert!(outcome.archived.is_none(), "archive must not run when project refuses");
        // Atomicity: plan.yaml must still exist on refusal.
        assert!(plan_path.exists(), "plan.yaml must remain on disk when finalize refuses");
    }

    #[test]
    fn passes_when_pr_is_merged() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        fs::write(tmp.path().join("plan.yaml"), "name: foo\nchanges: []\n").unwrap();

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 42,
                url: "u".to_string(),
            })),
        );
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::Merged);
        assert_eq!(outcome.summary.merged, 1);
    }

    #[test]
    fn passes_when_no_branch_for_project() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        fs::write(tmp.path().join("plan.yaml"), "name: foo\nchanges: []\n").unwrap();

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        // No `with_view` — defaults to Ok(None) i.e. no PR.
        let probe = MockProbe::new();
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::NoBranch);
        assert_eq!(outcome.summary.no_branch, 1);
    }

    #[test]
    fn refuses_on_branch_pattern_mismatch() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "feature/foo".to_string(),
                number: 1,
                url: "u".to_string(),
            })),
        );
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(!outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::BranchPatternMismatch);
        // Diagnostic must surface the literal expected branch.
        assert!(
            outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("specify/foo")),
            "branch-pattern-mismatch detail must include the expected branch, got: {:?}",
            outcome.projects[0].detail,
        );
    }

    #[test]
    fn refuses_on_gh_shell_error() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe =
            MockProbe::new().with_view("specify/foo", Err("simulated gh failure".to_string()));
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(!outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::Failed);
        assert!(outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("simulated")));
    }

    // ---- guard: dirty workspace ------------------------------------------

    #[test]
    fn refuses_dirty_workspace_without_clean() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let workspace_base = tmp.path().join(".specify/workspace");
        let alpha_path = workspace_base.join("alpha");
        fs::create_dir_all(&alpha_path).expect("mkdir alpha");

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Merged,
                    merged: true,
                    head_ref_name: "specify/foo".to_string(),
                    number: 42,
                    url: "u".to_string(),
                })),
            )
            .with_dirty(alpha_path, true);
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(!outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::Dirty);
        assert_eq!(outcome.projects[0].dirty, Some(true));
        assert!(
            outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("uncommitted")),
            "dirty diagnostic must mention uncommitted work, got: {:?}",
            outcome.projects[0].detail,
        );
        // Without --clean, the diagnostic should NOT mention --clean would drop work.
        assert!(
            !outcome.projects[0].detail.as_deref().unwrap_or("").contains("--clean"),
            "without --clean, diagnostic should not mention the --clean drop warning",
        );
    }

    #[test]
    fn refuses_dirty_workspace_with_clean() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let workspace_base = tmp.path().join(".specify/workspace");
        let alpha_path = workspace_base.join("alpha");
        fs::create_dir_all(&alpha_path).expect("mkdir alpha");

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new()
            .with_view(
                "specify/foo",
                Ok(Some(PrView {
                    state: PrState::Merged,
                    merged: true,
                    head_ref_name: "specify/foo".to_string(),
                    number: 42,
                    url: "u".to_string(),
                })),
            )
            .with_dirty(alpha_path.clone(), true);
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: true,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(!outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::Dirty);
        // With --clean, the diagnostic MUST mention that --clean would drop changes.
        assert!(
            outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("--clean")),
            "with --clean, diagnostic must warn about dropping changes, got: {:?}",
            outcome.projects[0].detail,
        );
        // Workspace clone must still exist — refused finalize never cleans.
        assert!(alpha_path.exists(), "refused --clean must leave clones alone");
    }

    // ---- dry-run --------------------------------------------------------

    #[test]
    fn dry_run_does_not_archive_or_clean() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let plan_path = tmp.path().join("plan.yaml");
        fs::write(&plan_path, "name: foo\nchanges: []\n").expect("seed plan");
        let workspace_base = tmp.path().join(".specify/workspace");
        let alpha_path = workspace_base.join("alpha");
        fs::create_dir_all(&alpha_path).expect("mkdir alpha");

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        );
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: true,
            dry_run: true,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(outcome.finalized, "dry-run with all-passing must report finalized=true");
        assert_eq!(outcome.dry_run, Some(true));
        assert!(outcome.archived.is_none(), "dry-run must not archive");
        assert!(outcome.cleaned.is_empty(), "dry-run must not clean");
        // On-disk state must be unchanged.
        assert!(plan_path.exists(), "dry-run must leave plan.yaml on disk");
        assert!(alpha_path.exists(), "dry-run must leave workspace clones");
    }

    #[test]
    fn dry_run_with_unmerged_pr_reports_not_finalized() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        );
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: true,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(!outcome.finalized);
        assert_eq!(outcome.projects[0].status, FinalizeStatus::Unmerged);
        assert_eq!(outcome.dry_run, Some(true));
    }

    // ---- --clean ---------------------------------------------------------

    #[test]
    fn clean_removes_clones_after_archive() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let plan_path = tmp.path().join("plan.yaml");
        fs::write(&plan_path, "name: foo\nchanges: []\n").expect("seed plan");
        let workspace_base = tmp.path().join(".specify/workspace");
        let alpha_path = workspace_base.join("alpha");
        fs::create_dir_all(&alpha_path).expect("mkdir alpha");
        // Drop a file inside so remove_dir_all has something to clear.
        fs::write(alpha_path.join("README.md"), "stub\n").expect("seed file");

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);
        let probe = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        );
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: true,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(outcome.finalized);
        assert_eq!(outcome.cleaned, vec!["alpha"], "alpha must be cleaned");
        assert!(!alpha_path.exists(), "workspace clone must be gone");
        assert!(!plan_path.exists(), "plan.yaml must be archived");
    }

    #[test]
    fn clean_skips_symlink_projects() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        fs::write(tmp.path().join("plan.yaml"), "name: foo\nchanges: []\n").unwrap();

        let plan = plan_named("foo");
        // url: "." → symlink-mode; clean must not delete the project_dir.
        let registry = Registry {
            version: 1,
            projects: vec![RegistryProject {
                name: "alpha".to_string(),
                url: ".".to_string(),
                schema: "omnia@v1".to_string(),
                description: Some("alpha service".to_string()),
                contracts: None,
            }],
        };
        let probe = MockProbe::new();
        let inputs = FinalizeInputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: true,
            dry_run: false,
        };
        let outcome = run_finalize(inputs, &probe).expect("ok");
        assert!(outcome.finalized);
        assert!(outcome.cleaned.is_empty(), "symlink projects must not be cleaned");
    }

    // ---- idempotency -----------------------------------------------------

    /// Operator runs finalize once with one PR open, gets refused.
    /// Operator merges the PR by hand. Operator runs finalize again —
    /// archive completes. The fixture verifies the second-run path.
    #[test]
    fn idempotent_after_manual_merge() {
        let tmp = TempDir::new().expect("tempdir");
        seed_specify_dir(tmp.path());
        let plan_path = tmp.path().join("plan.yaml");
        fs::write(&plan_path, "name: foo\nchanges: []\n").expect("seed plan");

        let plan = plan_named("foo");
        let registry = registry_with(&["alpha"]);

        // First run: PR open, finalize refuses.
        let probe1 = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        );
        let outcome1 = run_finalize(
            FinalizeInputs {
                project_dir: tmp.path(),
                plan: &plan,
                registry: &registry,
                clean: false,
                dry_run: false,
            },
            &probe1,
        )
        .expect("ok");
        assert!(!outcome1.finalized, "first run must refuse on unmerged PR");
        assert!(outcome1.archived.is_none());
        assert!(plan_path.exists(), "plan.yaml must still be present after refusal");

        // Operator merges the PR manually. Re-run finalize against a
        // probe that now reports MERGED — archive must land.
        let probe2 = MockProbe::new().with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        );
        let outcome2 = run_finalize(
            FinalizeInputs {
                project_dir: tmp.path(),
                plan: &plan,
                registry: &registry,
                clean: false,
                dry_run: false,
            },
            &probe2,
        )
        .expect("ok");
        assert!(outcome2.finalized, "second run after manual merge must finalize");
        assert!(outcome2.archived.is_some());
        assert!(!plan_path.exists(), "plan.yaml must be archived");
    }

    // ---- summary --------------------------------------------------------

    #[test]
    fn summary_counts_per_status() {
        let results = vec![
            FinalizeProjectResult {
                name: "a".into(),
                status: FinalizeStatus::Merged,
                pr_number: None,
                url: None,
                head_ref_name: None,
                dirty: None,
                detail: None,
            },
            FinalizeProjectResult {
                name: "b".into(),
                status: FinalizeStatus::NoBranch,
                pr_number: None,
                url: None,
                head_ref_name: None,
                dirty: None,
                detail: None,
            },
            FinalizeProjectResult {
                name: "c".into(),
                status: FinalizeStatus::Unmerged,
                pr_number: None,
                url: None,
                head_ref_name: None,
                dirty: None,
                detail: None,
            },
            FinalizeProjectResult {
                name: "d".into(),
                status: FinalizeStatus::Dirty,
                pr_number: None,
                url: None,
                head_ref_name: None,
                dirty: Some(true),
                detail: None,
            },
        ];
        let s = summarise(&results);
        assert_eq!(s.merged, 1);
        assert_eq!(s.no_branch, 1);
        assert_eq!(s.unmerged, 1);
        assert_eq!(s.dirty, 1);
    }

    // ---- helpers --------------------------------------------------------

    /// Seed `<tmp>/.specify/` so `Plan::archive` and friends have a
    /// real on-disk parent to operate on.
    fn seed_specify_dir(project_dir: &Path) {
        fs::create_dir_all(project_dir.join(".specify")).expect("mkdir .specify");
    }
}
