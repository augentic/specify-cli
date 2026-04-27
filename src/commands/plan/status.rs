use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{
    ChangeMetadata, Error, Plan, PlanChange, PlanStatus, PlanValidationLevel, ProjectConfig,
};

use super::{PlanRef, emit_structural_error, require_file};
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

#[allow(clippy::too_many_lines)]
pub fn run_plan_status(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let changes_dir = ProjectConfig::changes_dir(&ctx.project_dir);

    let results = plan.validate(Some(&changes_dir), None);
    let has_other_structural_errors = results
        .iter()
        .any(|r| matches!(r.level, PlanValidationLevel::Error) && r.code != "dependency-cycle");
    if has_other_structural_errors {
        return Ok(emit_structural_error(ctx.format));
    }

    let (ordered, order_label) = if let Ok(v) = plan.topological_order() {
        (v, "topological")
    } else {
        match ctx.format {
            OutputFormat::Json => {
                eprintln!(
                    "warning: dependency cycle detected — falling back to list order. Run 'specify plan validate' for detail."
                );
            }
            OutputFormat::Text => {
                println!(
                    "\u{26a0} dependency cycle detected — falling back to list order. Run 'specify plan validate' for detail."
                );
            }
        }
        (plan.changes.iter().collect::<Vec<_>>(), "list")
    };

    let mut counts: BTreeMap<PlanStatus, usize> = PlanStatus::ALL.iter().map(|&s| (s, 0)).collect();
    for entry in &plan.changes {
        *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
    }
    let total: usize = counts.values().sum();

    let active = plan.changes.iter().find(|c| c.status == PlanStatus::InProgress);
    let active_lifecycle = active.and_then(|a| read_lifecycle(&changes_dir.join(&a.name)));

    let blocked: Vec<&PlanChange> =
        plan.changes.iter().filter(|c| c.status == PlanStatus::Blocked).collect();
    let failed: Vec<&PlanChange> =
        plan.changes.iter().filter(|c| c.status == PlanStatus::Failed).collect();

    let next_eligible = plan.next_eligible();

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct StatusBody {
                plan: PlanRef,
                counts: Counts,
                order: &'static str,
                entries: Vec<Value>,
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

            let entries: Vec<Value> = ordered
                .iter()
                .map(|entry| {
                    let lifecycle = if entry.status == PlanStatus::InProgress {
                        active_lifecycle.clone()
                    } else {
                        None
                    };
                    plan_entry_to_json(entry, lifecycle)
                })
                .collect();

            let blocked_json: Vec<NameReason> = blocked
                .iter()
                .map(|c| NameReason {
                    name: c.name.clone(),
                    reason: c.status_reason.clone(),
                })
                .collect();
            let failed_json: Vec<NameReason> = failed
                .iter()
                .map(|c| NameReason {
                    name: c.name.clone(),
                    reason: c.status_reason.clone(),
                })
                .collect();

            let active_json = active.map(|a| Active {
                name: a.name.clone(),
                lifecycle: active_lifecycle.clone(),
            });

            emit_response(StatusBody {
                plan: PlanRef {
                    name: plan.name.clone(),
                    path: plan_path.display().to_string(),
                },
                counts: Counts {
                    done: counts[&PlanStatus::Done],
                    in_progress: counts[&PlanStatus::InProgress],
                    pending: counts[&PlanStatus::Pending],
                    blocked: counts[&PlanStatus::Blocked],
                    failed: counts[&PlanStatus::Failed],
                    skipped: counts[&PlanStatus::Skipped],
                    total,
                },
                order: order_label,
                entries,
                in_progress: active_json,
                blocked: blocked_json,
                failed: failed_json,
                next_eligible: next_eligible.map(|e| e.name.clone()),
            });
        }
        OutputFormat::Text => print_status(&StatusView {
            plan: &plan,
            counts: &counts,
            active,
            active_lifecycle: active_lifecycle.as_deref(),
            blocked: &blocked,
            failed: &failed,
            next_eligible,
        }),
    }
    Ok(CliResult::Success)
}

struct StatusView<'a> {
    plan: &'a Plan,
    counts: &'a BTreeMap<PlanStatus, usize>,
    active: Option<&'a PlanChange>,
    active_lifecycle: Option<&'a str>,
    blocked: &'a [&'a PlanChange],
    failed: &'a [&'a PlanChange],
    next_eligible: Option<&'a PlanChange>,
}

fn read_lifecycle(change_dir: &Path) -> Option<String> {
    if !ChangeMetadata::path(change_dir).exists() {
        return None;
    }
    ChangeMetadata::load(change_dir).ok().map(|m| m.status.to_string())
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

fn plan_entry_to_json(entry: &PlanChange, lifecycle: Option<String>) -> Value {
    serde_json::to_value(EntryRow {
        name: entry.name.clone(),
        status: entry.status.to_string(),
        depends_on: entry.depends_on.clone(),
        sources: entry.sources.clone(),
        status_reason: entry.status_reason.clone(),
        description: entry.description.clone(),
        lifecycle,
        context: entry.context.clone(),
    })
    .expect("EntryRow serialises")
}

fn print_status(view: &StatusView) {
    let counts = view.counts;
    let total: usize = counts.values().sum();
    println!("## Initiative: {}", view.plan.name);
    println!();
    println!();
    println!(
        "Progress: done {}, in-progress {}, pending {}, blocked {}, failed {}, skipped {} (total {total})",
        counts[&PlanStatus::Done],
        counts[&PlanStatus::InProgress],
        counts[&PlanStatus::Pending],
        counts[&PlanStatus::Blocked],
        counts[&PlanStatus::Failed],
        counts[&PlanStatus::Skipped],
    );

    if let Some(a) = view.active {
        let lifecycle_label = view.active_lifecycle.unwrap_or("<no change dir yet>");
        println!();
        println!("In progress: {} (lifecycle: {lifecycle_label})", a.name);
    }

    if !view.blocked.is_empty() {
        println!();
        println!("Blocked:");
        for c in view.blocked {
            let reason = c.status_reason.as_deref().unwrap_or("-");
            println!("  - {} (reason: {reason})", c.name);
        }
    }

    if !view.failed.is_empty() {
        println!();
        println!("Failed:");
        for c in view.failed {
            let reason = c.status_reason.as_deref().unwrap_or("-");
            println!("  - {} (reason: {reason})", c.name);
        }
    }

    println!();
    match view.next_eligible {
        Some(e) => println!("Next eligible: {}", e.name),
        None => println!("Next eligible: \u{2014} (waiting on dependencies / all done)"),
    }
}
