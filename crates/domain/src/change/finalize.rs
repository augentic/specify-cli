//! `specify change finalize` — verifies every per-project PR is merged
//! and every workspace clone is clean, then atomically archives
//! `plan.yaml`, `change.md`, and `.specify/plans/<name>/`.

#![allow(clippy::needless_pass_by_value)]

use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::change::plan::core::Plan;
use crate::cmd::CmdRunner;
use crate::config::Layout;
use crate::registry::Registry;
use crate::registry::forge::{SPECIFY_BRANCH_PREFIX, project_path};

mod archive;
mod probe;
mod summary;

pub use probe::{classify_pr, combine, is_dirty};
pub use summary::{is_terminal, outstanding, summarise};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-project classification for `specify change finalize`.
///
/// Display strings are kebab-case and match the JSON `status` value.
/// Skill authors and operators rely on this vocabulary; treat it as a
/// stable wire contract.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Landing {
    /// PR is `MERGED` on remote — passing.
    Merged,
    /// PR exists, branch matches, but has not been operator-merged
    /// (state `OPEN`). Refuses finalize.
    Unmerged,
    /// PR was `CLOSED` without merging. Refuses finalize.
    Closed,
    /// No PR on `specify/<change-name>` for this project — passing
    /// (e.g. the project was assigned no work in this change, or
    /// the operator merged via the forge UI / `gh pr merge` and deleted
    /// the branch).
    NoBranch,
    /// A PR exists but its `headRefName` is not the expected branch.
    /// Defence in depth — branch-pattern guard applies here too.
    BranchPatternMismatch,
    /// `git status --porcelain` for the workspace clone is non-empty.
    /// Refuses finalize even without `--clean`, to protect uncommitted
    /// work from a subsequent `--clean` run.
    Dirty,
    /// Generic shell-out failure (gh missing, unparseable JSON, network
    /// error, …). Refuses finalize.
    Failed,
}

impl Landing {
    /// Whether this per-project status counts as a passing classification
    /// for finalize purposes. Only `merged` and `no-branch` pass.
    #[must_use]
    pub const fn is_passing(self) -> bool {
        matches!(self, Self::Merged | Self::NoBranch)
    }
}

/// Per-project result row, surfaced in both text and JSON output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectResult {
    /// Registry project name.
    pub name: String,
    /// Outcome of the finalize attempt.
    #[serde(serialize_with = "serialize_status")]
    pub status: Landing,
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

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `serialize_with` signature requires `&T`."
)]
fn serialize_status<S: serde::Serializer>(status: &Landing, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(status.into())
}

/// Per-status counters for the summary row.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Summary {
    /// PRs in `MERGED` state on remote.
    pub merged: usize,
    /// PRs in `OPEN` state — refuses finalize.
    pub unmerged: usize,
    /// PRs in `CLOSED` state without merge — refuses finalize.
    pub closed: usize,
    /// Projects without a `specify/<change-name>` PR — passes.
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
pub struct Outcome {
    /// Change name (= `plan.yaml:name`).
    #[serde(rename = "change")]
    pub name: String,
    /// `true` when the archive landed on a real run, or when a dry-run
    /// preview classified the change as ready to finalize.
    pub finalized: bool,
    /// `specify/<change-name>` — surfaced for skill authors that
    /// echo the literal branch in operator-facing output.
    pub expected_branch: String,
    /// Per-project rows, one per registry entry.
    pub projects: Vec<ProjectResult>,
    /// Aggregate counts — same vocabulary as the per-project rows.
    pub summary: Summary,
    /// Operator-facing next step when finalize is refused. This is
    /// intentionally present in JSON too so non-text consumers can show
    /// the same operator guidance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Path of the archived `plan.yaml` (e.g.
    /// `.specify/archive/plans/foo-20260428.yaml`). `None` on dry-run
    /// or refused finalize.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<String>,
    /// Path of the archived `<name>-<date>/` directory when the plans
    /// working dir or `change.md` was co-moved. `None` when neither
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

/// Inputs that don't fit the per-project loop.
#[derive(Debug)]
pub struct Inputs<'a> {
    /// Project root directory (`.specify/` lives directly under here).
    pub project_dir: &'a Path,
    /// Loaded plan — owns the canonical change name.
    pub plan: &'a Plan,
    /// Loaded registry — owns the project list.
    pub registry: &'a Registry,
    /// `--clean` flag.
    pub clean: bool,
    /// `--dry-run` flag.
    pub dry_run: bool,
    /// Wall-clock instant supplied by the dispatcher; the archive sweep
    /// stamps the `<plan>-<YYYYMMDD>` segment from this value.
    pub now: Timestamp,
}

/// Top-level error sentinel for finalize.
///
/// Distinct from per-project failures: these are the **whole-run**
/// refusals (for example, non-terminal entries) that surface as a
/// hard error from the CLI handler with their own diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// One or more plan entries are not in a terminal state. Carries
    /// the offending entry names in plan order.
    NonTerminalEntries(Vec<String>),
}

/// Result of loading the optional finalize plan.
#[derive(Debug)]
pub enum PlanLoad {
    /// Plan file exists and parsed.
    Present(Plan),
    /// Plan file is absent; finalize treats this as already closed.
    Missing,
}

// ---------------------------------------------------------------------------
// Orchestration — generic over Probe for testability
// ---------------------------------------------------------------------------

/// Run the whole finalize pipeline.
///
/// Order:
/// 1. Plan-presence guard (caller's responsibility — call
///    [`load_plan`] first; the `Plan` arrives here loaded).
/// 2. Plan terminal-state guard (returns
///    [`Refusal::NonTerminalEntries`] when not satisfied).
/// 3. Per-project probes — PR state + dirty clone.
/// 4. When all projects pass and not `--dry-run`: archive plan + clean.
/// 5. Always returns an [`Outcome`] for consumers; a refused
///    finalize has `finalized: false` and pinpoints the failing
///    projects.
///
/// # Errors
///
/// Returns [`Refusal`] for whole-run refusals. Per-project
/// failures live in [`Outcome::projects`] and never bubble up.
pub fn run<R: CmdRunner>(inputs: Inputs<'_>, runner: &R) -> Result<Outcome, Refusal> {
    // Refuse if any plan entry is still in a non-terminal state.
    let outstanding = summary::outstanding(inputs.plan);
    if !outstanding.is_empty() {
        return Err(Refusal::NonTerminalEntries(outstanding));
    }

    let change_name = inputs.plan.name.clone();
    let expected_branch = format!("{SPECIFY_BRANCH_PREFIX}{change_name}");
    let workspace_base = Layout::new(inputs.project_dir).specify_dir().join("workspace");

    // Probe per-project PR state + dirty clones.
    let mut projects: Vec<ProjectResult> = Vec::with_capacity(inputs.registry.projects.len());
    for rp in &inputs.registry.projects {
        let path = project_path(inputs.project_dir, &workspace_base, rp);
        projects.push(probe::probe_one(runner, &path, rp, &expected_branch, inputs.clean));
    }

    let aggregated = summary::summarise(&projects);
    let any_refusing = projects.iter().any(|p| !p.status.is_passing());

    let mut outcome = Outcome {
        name: change_name,
        finalized: false,
        expected_branch,
        projects,
        summary: aggregated,
        message: None,
        archived: None,
        archived_plans_dir: None,
        cleaned: Vec::new(),
        dry_run: inputs.dry_run.then_some(true),
    };

    if any_refusing {
        outcome.message = summary::refusal_message(&outcome.summary, &outcome.expected_branch);
        return Ok(outcome);
    }

    // Dry-run: preview only. Don't archive, don't clean.
    if inputs.dry_run {
        outcome.finalized = true;
        return Ok(outcome);
    }

    // All guards passed — archive (atomic) and optionally clean.
    if !archive::sweep(inputs.project_dir, &mut outcome, inputs.now) {
        return Ok(outcome);
    }

    if inputs.clean {
        outcome.cleaned = archive::clean_clones(&workspace_base, inputs.registry);
    }

    outcome.finalized = true;
    Ok(outcome)
}

/// Plan-presence guard: load `plan.yaml` (at the repo root) or return
/// [`PlanLoad::Missing`].
///
/// # Errors
///
/// Bubbles up `Plan::load` errors verbatim — a malformed plan is a
/// real failure, not a "plan absent" sentinel.
pub fn load_plan(project_dir: &Path) -> Result<PlanLoad, Error> {
    let plan_file = Layout::new(project_dir).plan_path();
    if !plan_file.exists() {
        return Ok(PlanLoad::Missing);
    }
    Ok(PlanLoad::Present(Plan::load(&plan_file)?))
}
