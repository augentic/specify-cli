//! Atomic plan/brief/plans-dir sweep + workspace-clone cleanup.
//!
//! `Plan::archive` preflights both destinations (`<name>-<date>.yaml`
//! and `<name>-<date>/`) before any move, so a collision returns an
//! error before any file is touched. `clean_clones` only runs after
//! the archive has succeeded, so a failed archive leaves clones intact.

use std::path::Path;

use chrono::{DateTime, Utc};
use crate::config::LayoutExt;
use crate::registry::Registry;

use super::{Landing, Outcome, ProjectResult};
use crate::change::plan::core::Plan;

/// Try to archive `plan.yaml`, the change brief, and the plans working
/// dir into `.specify/archive/plans/`. On success, populates
/// `outcome.archived` and `outcome.archived_plans_dir` and returns
/// `true`. On failure, stamps an explanatory `detail` onto the first
/// project row (or a synthetic `<archive>` row when the registry is
/// empty), records a summary message, and returns `false`.
pub(super) fn sweep(project_dir: &Path, outcome: &mut Outcome, now: DateTime<Utc>) -> bool {
    let layout = project_dir.layout();
    let plan_file = layout.plan_path();
    let brief_file = layout.change_brief_path();
    let archive_root = layout.archive_dir().join("plans");
    match Plan::archive(&plan_file, &brief_file, &archive_root, /* force = */ true, now) {
        Ok((archived, archived_plans_dir)) => {
            outcome.archived = Some(archived.display().to_string());
            outcome.archived_plans_dir =
                archived_plans_dir.as_ref().map(|p| p.display().to_string());
            true
        }
        Err(err) => {
            // Atomicity: Plan::archive preflights both destinations
            // before any move, so a failure here leaves the on-disk
            // state untouched. Surface the cause via `detail` and
            // keep finalized=false (the load-bearing signal).
            outcome.message =
                Some("plan archive failed; workspace clones were not cleaned".to_string());
            let detail = format!("plan archive failed: {err}");
            if let Some(first) = outcome.projects.first_mut() {
                first.detail = Some(detail);
            } else {
                outcome.projects.push(ProjectResult {
                    name: "<archive>".to_string(),
                    status: Landing::Failed,
                    pr_number: None,
                    url: None,
                    head_ref_name: None,
                    dirty: None,
                    detail: Some(detail),
                });
            }
            false
        }
    }
}

/// Remove `.specify/workspace/<name>/` clones for every non-symlink
/// registry project. Best-effort: a single project's failure is
/// recorded silently (the archive has already landed; clean is the
/// optional bonus step). Returns the names of successfully-cleaned
/// projects so the caller can surface them.
pub(super) fn clean_clones(workspace_base: &Path, registry: &Registry) -> Vec<String> {
    let mut cleaned = Vec::new();
    for rp in &registry.projects {
        // Symlink projects point at source repositories the operator
        // owns separately — never delete them on `--clean`.
        if rp.is_local() {
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
