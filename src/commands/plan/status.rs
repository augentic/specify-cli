use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_domain::change::{Entry, Lifecycle, Plan, Severity, SliceSourceBinding, Status};
use specify_domain::slice::{LifecycleStatus, SliceMetadata};
use specify_error::{Error, Result};

use super::{Ref, require_file};
use crate::context::Ctx;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct StatusBody {
    plan: Ref,
    lifecycle: Lifecycle,
    drained: bool,
    counts: Counts,
    order: OrderLabel,
    entries: Vec<EntryRow>,
    in_progress: Option<Active>,
    next_eligible: Option<String>,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum OrderLabel {
    Topological,
    List,
}

/// Per-entry status counts. Post-RFC-25 only three states ship — the
/// counts surface mirrors the surviving enum.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Counts {
    pub done: usize,
    pub in_progress: usize,
    pub pending: usize,
    pub total: usize,
}

impl Counts {
    pub fn from_entries(entries: &[Entry]) -> Self {
        let mut c = Self {
            done: 0,
            in_progress: 0,
            pending: 0,
            total: entries.len(),
        };
        for e in entries {
            match e.status {
                Status::Done => c.done += 1,
                Status::InProgress => c.in_progress += 1,
                Status::Pending => c.pending += 1,
            }
        }
        c
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Active {
    name: String,
    lifecycle: Option<LifecycleStatus>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryRow {
    name: String,
    status: Status,
    depends_on: Vec<String>,
    sources: Vec<SliceSourceBinding>,
    description: Option<String>,
    lifecycle: Option<LifecycleStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<String>,
}

fn write_status_text(w: &mut dyn Write, body: &StatusBody) -> std::io::Result<()> {
    let c = &body.counts;
    writeln!(w, "## Change: {}", body.plan.name)?;
    writeln!(w)?;
    writeln!(w, "Plan lifecycle: {}", body.lifecycle)?;
    writeln!(w, "Drained: {}", body.drained)?;
    writeln!(w)?;
    writeln!(
        w,
        "Progress: done {}, in-progress {}, pending {} (total {})",
        c.done, c.in_progress, c.pending, c.total,
    )?;

    if let Some(a) = &body.in_progress {
        let lifecycle_label =
            a.lifecycle.map_or_else(|| "<no slice dir yet>".to_string(), |l| l.to_string());
        writeln!(w)?;
        writeln!(w, "In progress: {} (lifecycle: {lifecycle_label})", a.name)?;
    }

    writeln!(w)?;
    match &body.next_eligible {
        Some(name) => writeln!(w, "Next eligible: {name}")?,
        None => writeln!(w, "Next eligible: \u{2014} (waiting on dependencies / all done)")?,
    }
    Ok(())
}

pub(super) fn run(ctx: &Ctx) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ctx.slices_dir();

    let results = plan.validate(Some(&slices_dir), None);
    let has_other_structural_errors =
        results.iter().any(|r| matches!(r.level, Severity::Error) && r.code != "dependency-cycle");
    if has_other_structural_errors {
        return Err(Error::validation_failed(
            "plan-structural-errors",
            "plan must be free of structural errors",
            "run 'specify plan validate' for detail",
        ));
    }

    let (ordered, order_label) = if let Ok(v) = plan.topological_order() {
        (v, OrderLabel::Topological)
    } else {
        eprintln!(
            "warning: dependency cycle detected — falling back to list order. Run 'specify plan validate' for detail."
        );
        (plan.entries.iter().collect::<Vec<_>>(), OrderLabel::List)
    };

    let counts = Counts::from_entries(&plan.entries);

    let active = plan.entries.iter().find(|c| c.status == Status::InProgress);
    let active_lifecycle = active.and_then(|a| read_lifecycle(&slices_dir.join(&a.name)));

    let entries: Vec<EntryRow> = ordered
        .iter()
        .map(|entry| {
            let lifecycle =
                if entry.status == Status::InProgress { active_lifecycle } else { None };
            entry_row(entry, lifecycle)
        })
        .collect();

    let in_progress = active.map(|a| Active {
        name: a.name.clone(),
        lifecycle: active_lifecycle,
    });

    let body = StatusBody {
        plan: Ref {
            name: plan.name.clone(),
            path: plan_path.display().to_string(),
        },
        lifecycle: plan.lifecycle,
        drained: plan.is_drained(),
        counts,
        order: order_label,
        entries,
        in_progress,
        next_eligible: plan.next_eligible().map(|e| e.name.clone()),
    };

    ctx.write(&body, write_status_text)?;
    Ok(())
}

fn entry_row(entry: &Entry, lifecycle: Option<LifecycleStatus>) -> EntryRow {
    EntryRow {
        name: entry.name.clone(),
        status: entry.status,
        depends_on: entry.depends_on.clone(),
        sources: entry.sources.clone(),
        description: entry.description.clone(),
        lifecycle,
        context: entry.context.clone(),
    }
}

fn read_lifecycle(slice_dir: &Path) -> Option<LifecycleStatus> {
    if !SliceMetadata::path(slice_dir).exists() {
        return None;
    }
    SliceMetadata::load(slice_dir).ok().map(|m| m.status)
}
