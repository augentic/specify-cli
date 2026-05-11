use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_change::{Entry, Plan, Severity, Status};
use specify_config::ProjectConfig;
use specify_error::{Error, Result};
use specify_slice::SliceMetadata;

use super::{PlanRef, require_file};
use crate::context::CommandContext;
use crate::output::{Render, emit};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct StatusBody {
    plan: PlanRef,
    counts: Counts,
    order: &'static str,
    entries: Vec<EntryRow>,
    in_progress: Option<Active>,
    blocked: Vec<NameReason>,
    failed: Vec<NameReason>,
    next_eligible: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Counts {
    done: usize,
    in_progress: usize,
    pending: usize,
    blocked: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Active {
    name: String,
    lifecycle: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct NameReason {
    name: String,
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryRow {
    name: String,
    status: String,
    depends_on: Vec<String>,
    sources: Vec<String>,
    status_reason: Option<String>,
    description: Option<String>,
    lifecycle: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<String>,
}

impl Render for StatusBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let c = &self.counts;
        writeln!(w, "## Change: {}", self.plan.name)?;
        writeln!(w)?;
        writeln!(w)?;
        writeln!(
            w,
            "Progress: done {}, in-progress {}, pending {}, blocked {}, failed {}, skipped {} (total {})",
            c.done, c.in_progress, c.pending, c.blocked, c.failed, c.skipped, c.total,
        )?;

        if let Some(a) = &self.in_progress {
            let lifecycle_label = a.lifecycle.as_deref().unwrap_or("<no slice dir yet>");
            writeln!(w)?;
            writeln!(w, "In progress: {} (lifecycle: {lifecycle_label})", a.name)?;
        }

        if !self.blocked.is_empty() {
            writeln!(w)?;
            writeln!(w, "Blocked:")?;
            for row in &self.blocked {
                let reason = row.reason.as_deref().unwrap_or("-");
                writeln!(w, "  - {} (reason: {reason})", row.name)?;
            }
        }

        if !self.failed.is_empty() {
            writeln!(w)?;
            writeln!(w, "Failed:")?;
            for row in &self.failed {
                let reason = row.reason.as_deref().unwrap_or("-");
                writeln!(w, "  - {} (reason: {reason})", row.name)?;
            }
        }

        writeln!(w)?;
        match &self.next_eligible {
            Some(name) => writeln!(w, "Next eligible: {name}")?,
            None => writeln!(w, "Next eligible: \u{2014} (waiting on dependencies / all done)")?,
        }
        Ok(())
    }
}

pub fn run(ctx: &CommandContext) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ProjectConfig::slices_dir(&ctx.project_dir);

    let results = plan.validate(Some(&slices_dir), None);
    let has_other_structural_errors =
        results.iter().any(|r| matches!(r.level, Severity::Error) && r.code != "dependency-cycle");
    if has_other_structural_errors {
        return Err(Error::PlanStructural);
    }

    let (ordered, order_label) = if let Ok(v) = plan.topological_order() {
        (v, "topological")
    } else {
        eprintln!(
            "warning: dependency cycle detected — falling back to list order. Run 'specify change plan validate' for detail."
        );
        (plan.entries.iter().collect::<Vec<_>>(), "list")
    };

    let mut counts: BTreeMap<Status, usize> = Status::ALL.iter().map(|&s| (s, 0)).collect();
    for entry in &plan.entries {
        *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
    }
    let total: usize = counts.values().sum();

    let active = plan.entries.iter().find(|c| c.status == Status::InProgress);
    let active_lifecycle = active.and_then(|a| read_lifecycle(&slices_dir.join(&a.name)));

    let entries: Vec<EntryRow> = ordered
        .iter()
        .map(|entry| {
            let lifecycle =
                if entry.status == Status::InProgress { active_lifecycle.clone() } else { None };
            entry_row(entry, lifecycle)
        })
        .collect();

    let blocked: Vec<NameReason> =
        plan.entries.iter().filter(|c| c.status == Status::Blocked).map(name_reason).collect();
    let failed: Vec<NameReason> =
        plan.entries.iter().filter(|c| c.status == Status::Failed).map(name_reason).collect();

    let in_progress = active.map(|a| Active {
        name: a.name.clone(),
        lifecycle: active_lifecycle.clone(),
    });

    let body = StatusBody {
        plan: PlanRef {
            name: plan.name.clone(),
            path: plan_path.display().to_string(),
        },
        counts: Counts {
            done: counts[&Status::Done],
            in_progress: counts[&Status::InProgress],
            pending: counts[&Status::Pending],
            blocked: counts[&Status::Blocked],
            failed: counts[&Status::Failed],
            skipped: counts[&Status::Skipped],
            total,
        },
        order: order_label,
        entries,
        in_progress,
        blocked,
        failed,
        next_eligible: plan.next_eligible().map(|e| e.name.clone()),
    };

    emit(ctx.format, &body)?;
    Ok(())
}

fn name_reason(entry: &Entry) -> NameReason {
    NameReason {
        name: entry.name.clone(),
        reason: entry.status_reason.clone(),
    }
}

fn entry_row(entry: &Entry, lifecycle: Option<String>) -> EntryRow {
    EntryRow {
        name: entry.name.clone(),
        status: entry.status.to_string(),
        depends_on: entry.depends_on.clone(),
        sources: entry.sources.clone(),
        status_reason: entry.status_reason.clone(),
        description: entry.description.clone(),
        lifecycle,
        context: entry.context.clone(),
    }
}

fn read_lifecycle(slice_dir: &Path) -> Option<String> {
    if !SliceMetadata::path(slice_dir).exists() {
        return None;
    }
    SliceMetadata::load(slice_dir).ok().map(|m| m.status.to_string())
}
