#![allow(clippy::needless_pass_by_value)]

//! Top-level `specify status` — project dashboard.
//!
//! Aggregates the registry summary, plan progress counts, and the
//! active-change list. Single-change status lives in
//! `super::change::ChangeAction::Status`; this module is dashboard-only.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, Plan, PlanStatus, ProjectConfig, Registry};

use super::change::{collect_status, list_change_names, status_entry_to_json};
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_status_dashboard(ctx: &CommandContext) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let changes_dir = ctx.changes_dir();

    let registry = Registry::load(&ctx.project_dir)?;
    let plan_summary = load_plan_summary(ctx);

    let names = list_change_names(&changes_dir)?;
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let dir = changes_dir.join(&name);
        entries.push(collect_status(&dir, &name, &pipeline, &ctx.project_dir)?);
    }

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct DashboardBody {
                registry: Value,
                plan: Value,
                changes: Vec<Value>,
            }
            let registry_json = registry
                .map_or(Value::Null, |r| serde_json::to_value(r).expect("Registry serialises"));
            let plan_json = plan_summary
                .as_ref()
                .map_or(Value::Null, |p| serde_json::to_value(p).expect("PlanSummary serialises"));
            let changes_json: Vec<Value> = entries.iter().map(status_entry_to_json).collect();
            emit_response(DashboardBody {
                registry: registry_json,
                plan: plan_json,
                changes: changes_json,
            });
        }
        OutputFormat::Text => {
            print_dashboard_text(registry.as_ref(), plan_summary.as_ref(), &entries);
        }
    }
    Ok(CliResult::Success)
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
    let plan_path = ProjectConfig::specify_dir(&ctx.project_dir).join("plan.yaml");
    if !plan_path.exists() {
        return None;
    }
    let plan = Plan::load(&plan_path).ok()?;

    let mut counts: BTreeMap<PlanStatus, usize> = PlanStatus::ALL.iter().map(|&s| (s, 0)).collect();
    for entry in &plan.changes {
        *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
    }
    let total: usize = counts.values().sum();

    Some(PlanSummary {
        name: plan.name,
        counts: PlanCounts {
            done: counts[&PlanStatus::Done],
            in_progress: counts[&PlanStatus::InProgress],
            pending: counts[&PlanStatus::Pending],
            blocked: counts[&PlanStatus::Blocked],
            failed: counts[&PlanStatus::Failed],
            skipped: counts[&PlanStatus::Skipped],
            total,
        },
    })
}

fn print_dashboard_text(
    registry: Option<&Registry>, plan: Option<&PlanSummary>, entries: &[super::change::StatusEntry],
) {
    println!("## Registry");
    match registry {
        None => println!("  (no registry declared)"),
        Some(r) => {
            println!("  version: {}", r.version);
            if r.projects.is_empty() {
                println!("  projects: (none)");
            } else {
                println!("  projects ({}):", r.projects.len());
                for p in &r.projects {
                    println!("    - {} ({})", p.name, p.schema);
                }
            }
        }
    }

    println!();
    println!("## Plan");
    match plan {
        None => println!("  (no plan)"),
        Some(p) => {
            println!("  name: {}", p.name);
            println!(
                "  progress: done {}, in-progress {}, pending {}, blocked {}, failed {}, skipped {} (total {})",
                p.counts.done,
                p.counts.in_progress,
                p.counts.pending,
                p.counts.blocked,
                p.counts.failed,
                p.counts.skipped,
                p.counts.total,
            );
        }
    }

    println!();
    println!("## Active changes");
    if entries.is_empty() {
        println!("  (none)");
        return;
    }
    let name_w = entries.iter().map(|e| e.name.len()).max().unwrap_or(6).max(6);
    let status_w = entries.iter().map(|e| e.status.len()).max().unwrap_or(6).max(6);
    println!(
        "  {:<name_w$}  {:<status_w$}  tasks",
        "change",
        "status",
        name_w = name_w,
        status_w = status_w
    );
    for e in entries {
        let tasks = match e.tasks {
            Some((complete, total)) => format!("{complete}/{total}"),
            None => "-".to_string(),
        };
        println!(
            "  {:<name_w$}  {:<status_w$}  {}",
            e.name,
            e.status,
            tasks,
            name_w = name_w,
            status_w = status_w
        );
    }
}
