//! Top-level `specify status` — project dashboard.
//!
//! Aggregates the registry summary, plan progress counts, and the
//! active-slice list. Single-slice status lives in
//! `super::slice::SliceAction::Status`; this module is dashboard-only.

use std::collections::BTreeMap;
use std::io::Write;

use serde::Serialize;
use serde_json::Value;
use specify_change::{Plan, Status};
use specify_config::ProjectConfig;
use specify_error::Result;
use specify_registry::Registry;

use super::slice::{StatusEntry, collect_status, list_slice_names, status_entry_to_json};
use crate::context::CommandContext;
use crate::output::{Render, emit};

pub fn run(ctx: &CommandContext) -> Result<()> {
    let pipeline = ctx.load_pipeline()?;
    let slices_dir = ctx.slices_dir();

    let registry = Registry::load(&ctx.project_dir)?;
    let plan_summary = load_plan_summary(ctx);

    let names = list_slice_names(&slices_dir)?;
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let dir = slices_dir.join(&name);
        entries.push(collect_status(&dir, &name, &pipeline, &ctx.project_dir)?);
    }

    let body = DashboardBody::new(registry, plan_summary, entries);
    emit(ctx.format, &body)?;
    Ok(())
}

struct DashboardBody {
    registry: Option<Registry>,
    plan: Option<PlanSummary>,
    entries: Vec<StatusEntry>,
}

impl DashboardBody {
    const fn new(
        registry: Option<Registry>, plan: Option<PlanSummary>, entries: Vec<StatusEntry>,
    ) -> Self {
        Self {
            registry,
            plan,
            entries,
        }
    }
}

impl Serialize for DashboardBody {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let registry_json = self
            .registry
            .as_ref()
            .map_or(Value::Null, |r| serde_json::to_value(r).expect("Registry serialises"));
        let plan_json = self
            .plan
            .as_ref()
            .map_or(Value::Null, |p| serde_json::to_value(p).expect("PlanSummary serialises"));
        let slices_json: Vec<Value> = self.entries.iter().map(status_entry_to_json).collect();
        let mut s = serializer.serialize_struct("DashboardBody", 3)?;
        s.serialize_field("registry", &registry_json)?;
        s.serialize_field("plan", &plan_json)?;
        s.serialize_field("slices", &slices_json)?;
        s.end()
    }
}

impl Render for DashboardBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        render_dashboard(w, self.registry.as_ref(), self.plan.as_ref(), &self.entries)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanSummary {
    name: String,
    counts: PlanCounts,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanCounts {
    done: usize,
    in_progress: usize,
    pending: usize,
    blocked: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

fn load_plan_summary(ctx: &CommandContext) -> Option<PlanSummary> {
    let plan_path = ProjectConfig::plan_path(&ctx.project_dir);
    if !plan_path.exists() {
        return None;
    }
    let plan = Plan::load(&plan_path).ok()?;

    let mut counts: BTreeMap<Status, usize> = Status::ALL.iter().map(|&s| (s, 0)).collect();
    for entry in &plan.entries {
        *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
    }
    let total: usize = counts.values().sum();

    Some(PlanSummary {
        name: plan.name,
        counts: PlanCounts {
            done: counts[&Status::Done],
            in_progress: counts[&Status::InProgress],
            pending: counts[&Status::Pending],
            blocked: counts[&Status::Blocked],
            failed: counts[&Status::Failed],
            skipped: counts[&Status::Skipped],
            total,
        },
    })
}

fn render_dashboard(
    w: &mut dyn Write, registry: Option<&Registry>, plan: Option<&PlanSummary>,
    entries: &[StatusEntry],
) -> std::io::Result<()> {
    writeln!(w, "## Registry")?;
    match registry {
        None => writeln!(w, "  (no registry declared)")?,
        Some(r) => {
            writeln!(w, "  version: {}", r.version)?;
            if r.projects.is_empty() {
                writeln!(w, "  projects: (none)")?;
            } else {
                writeln!(w, "  projects ({}):", r.projects.len())?;
                for p in &r.projects {
                    writeln!(w, "    - {} ({})", p.name, p.capability)?;
                }
            }
        }
    }

    writeln!(w)?;
    writeln!(w, "## Plan")?;
    match plan {
        None => writeln!(w, "  (no plan)")?,
        Some(p) => {
            writeln!(w, "  name: {}", p.name)?;
            writeln!(
                w,
                "  progress: done {}, in-progress {}, pending {}, blocked {}, failed {}, skipped {} (total {})",
                p.counts.done,
                p.counts.in_progress,
                p.counts.pending,
                p.counts.blocked,
                p.counts.failed,
                p.counts.skipped,
                p.counts.total,
            )?;
        }
    }

    writeln!(w)?;
    writeln!(w, "## Active slices")?;
    if entries.is_empty() {
        writeln!(w, "  (none)")?;
        return Ok(());
    }
    let name_w = entries.iter().map(|e| e.name.len()).max().unwrap_or(6).max(6);
    let status_w = entries.iter().map(|e| e.status.len()).max().unwrap_or(6).max(6);
    writeln!(
        w,
        "  {:<name_w$}  {:<status_w$}  tasks",
        "slice",
        "status",
        name_w = name_w,
        status_w = status_w
    )?;
    for e in entries {
        let tasks = match e.tasks {
            Some((complete, total)) => format!("{complete}/{total}"),
            None => "-".to_string(),
        };
        writeln!(
            w,
            "  {:<name_w$}  {:<status_w$}  {}",
            e.name,
            e.status,
            tasks,
            name_w = name_w,
            status_w = status_w
        )?;
    }
    Ok(())
}
