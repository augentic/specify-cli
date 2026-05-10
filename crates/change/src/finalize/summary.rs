//! Aggregation helpers — terminal-state predicate, outstanding entries,
//! per-status counts, and the operator-facing refusal-message builder.

use super::{ProjectResult, Summary};
use crate::plan::core::{Plan, Status};

/// Whether a plan-entry status counts as terminal for finalize.
///
/// Per the brief, `done` / `failed` / `dropped` are terminal; the
/// in-`Plan` representation maps `dropped` to [`Status::Skipped`]
/// (the latter is what `specify change drop` surfaces back to the
/// plan).
#[must_use]
pub const fn is_terminal(status: Status) -> bool {
    matches!(status, Status::Done | Status::Failed | Status::Skipped)
}

/// Walk the plan and return the names of entries whose status is not a
/// terminal-for-finalize state. List order matches plan order so the
/// diagnostic is stable.
#[must_use]
pub fn outstanding(plan: &Plan) -> Vec<String> {
    plan.entries.iter().filter(|c| !is_terminal(c.status)).map(|c| c.name.clone()).collect()
}

/// Aggregate per-status counts for the summary row.
#[must_use]
pub fn summarise(results: &[ProjectResult]) -> Summary {
    let mut s = Summary::default();
    for r in results {
        match r.status {
            super::Landing::Merged => s.merged += 1,
            super::Landing::Unmerged => s.unmerged += 1,
            super::Landing::Closed => s.closed += 1,
            super::Landing::NoBranch => s.no_branch += 1,
            super::Landing::BranchPatternMismatch => s.branch_pattern_mismatch += 1,
            super::Landing::Dirty => s.dirty += 1,
            super::Landing::Failed => s.failed += 1,
        }
    }
    s
}

/// Build the operator-facing summary message for a refused finalize run.
/// Returns `None` when no refusing-status counter is non-zero.
pub(super) fn refusal_message(summary: &Summary, expected_branch: &str) -> Option<String> {
    let mut guidance: Vec<String> = Vec::new();
    if summary.unmerged > 0 {
        guidance.push(format!(
            "{} unmerged PR(s) must be operator-merged through the forge UI or `gh pr merge` before finalize",
            summary.unmerged,
        ));
    }
    if summary.closed > 0 {
        guidance.push(format!(
            "{} closed PR(s) were not merged; reopen or push a replacement on `{expected_branch}` and operator-merge before finalize",
            summary.closed,
        ));
    }
    if summary.branch_pattern_mismatch > 0 {
        guidance.push(format!(
            "{} PR(s) have the wrong head branch; recreate them from `{expected_branch}` before finalize",
            summary.branch_pattern_mismatch,
        ));
    }
    if summary.dirty > 0 {
        guidance.push(format!(
            "{} dirty workspace clone(s) must be committed, pushed, or stashed before finalize can archive or clean",
            summary.dirty,
        ));
    }
    if summary.failed > 0 {
        guidance.push(format!(
            "{} PR or workspace probe failure(s) must be resolved before finalize can continue",
            summary.failed,
        ));
    }

    (!guidance.is_empty()).then(|| guidance.join("; "))
}
