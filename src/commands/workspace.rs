#![allow(clippy::needless_pass_by_value, clippy::option_if_let_else)]

use serde::Serialize;
use serde_json::Value;
use specify::{
    Error, Plan, PushOutcome, Registry, SlotKind, SlotStatus, sync_registry_workspace,
    workspace_status,
};

use super::plan::require_file;
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_workspace_sync(ctx: &CommandContext) -> Result<CliResult, Error> {
    match Registry::load(&ctx.project_dir)? {
        None => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct SyncAbsent {
                        registry: Value,
                        synced: bool,
                        message: &'static str,
                    }
                    emit_response(SyncAbsent {
                        registry: Value::Null,
                        synced: false,
                        message: "no registry declared at .specify/registry.yaml; nothing to sync",
                    });
                }
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml; nothing to sync");
                }
            }
            Ok(CliResult::Success)
        }
        Some(registry) => {
            sync_registry_workspace(&ctx.project_dir)?;
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct SyncBody {
                        registry: Registry,
                        synced: bool,
                    }
                    emit_response(SyncBody {
                        registry,
                        synced: true,
                    });
                }
                OutputFormat::Text => println!("workspace sync complete"),
            }
            Ok(CliResult::Success)
        }
    }
}

pub fn run_workspace_status(ctx: &CommandContext) -> Result<CliResult, Error> {
    match workspace_status(&ctx.project_dir)? {
        None => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct StatusAbsent {
                        registry: Value,
                        slots: Value,
                    }
                    emit_response(StatusAbsent {
                        registry: Value::Null,
                        slots: Value::Null,
                    });
                }
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml");
                }
            }
            Ok(CliResult::Success)
        }
        Some(slots) => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct StatusBody {
                        slots: Vec<Value>,
                    }
                    let items: Vec<Value> = slots.iter().map(slot_to_json).collect();
                    emit_response(StatusBody { slots: items });
                }
                OutputFormat::Text => {
                    for slot in &slots {
                        print_slot(slot);
                    }
                }
            }
            Ok(CliResult::Success)
        }
    }
}

const fn kind_label(kind: SlotKind) -> &'static str {
    match kind {
        SlotKind::Missing => "missing",
        SlotKind::Symlink => "symlink",
        SlotKind::GitClone => "git-clone",
        SlotKind::Other => "other",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SlotJson {
    name: String,
    kind: &'static str,
    head_sha: Option<String>,
    dirty: Option<bool>,
}

fn slot_to_json(slot: &SlotStatus) -> Value {
    serde_json::to_value(SlotJson {
        name: slot.name.clone(),
        kind: kind_label(slot.kind),
        head_sha: slot.head_sha.clone(),
        dirty: slot.dirty,
    })
    .expect("SlotJson serialises")
}

fn print_slot(slot: &SlotStatus) {
    let kind = kind_label(slot.kind);
    let head = slot.head_sha.as_deref().unwrap_or("-");
    let dirty = match slot.dirty {
        None => "-",
        Some(true) => "yes",
        Some(false) => "no",
    };
    println!("{}: kind={kind} head={head} dirty={dirty}", slot.name);
}

pub fn run_workspace_push(
    ctx: &CommandContext, projects: Vec<String>, dry_run: bool,
) -> Result<CliResult, Error> {
    let plan_path = require_file(&ctx.project_dir).map_err(|_err| {
        Error::Config(
            "No active plan found at .specify/plan.yaml. Run 'specify plan init' \
             to create one, or check whether the plan was already archived."
                .to_string(),
        )
    })?;
    let plan = Plan::load(&plan_path)?;

    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::Config(
            "No registry.yaml found; workspace push requires a registry".to_string(),
        ));
    };

    let results =
        specify::run_workspace_push_impl(&ctx.project_dir, &plan, &registry, &projects, dry_run)?;

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PushBody {
                projects: Vec<PushItem>,
                #[serde(skip_serializing_if = "Option::is_none")]
                dry_run: Option<bool>,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PushItem {
                name: String,
                status: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                branch: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                pr: Option<u64>,
            }
            let items: Vec<PushItem> = results
                .iter()
                .map(|r| PushItem {
                    name: r.name.clone(),
                    status: r.status.to_string(),
                    branch: r.branch.clone(),
                    pr: r.pr_number,
                })
                .collect();
            emit_response(PushBody {
                projects: items,
                dry_run: dry_run.then_some(true),
            });
        }
        OutputFormat::Text => {
            if dry_run {
                println!("[dry-run] specify: workspace push — {}", plan.name);
            } else {
                println!("specify: workspace push — {}", plan.name);
            }
            println!();
            for r in &results {
                let status_label =
                    if dry_run && matches!(r.status, PushOutcome::Pushed | PushOutcome::Created) {
                        format!("would-{}", r.status)
                    } else {
                        r.status.to_string()
                    };
                let branch_part = r.branch.as_deref().unwrap_or("");
                let pr_part = r.pr_number.map(|n| format!("PR #{n}")).unwrap_or_default();
                println!("  {:<20} {:<14} {} {}", r.name, status_label, branch_part, pr_part);
            }
            let created = results.iter().filter(|r| r.status == PushOutcome::Created).count();
            let pushed = results.iter().filter(|r| r.status == PushOutcome::Pushed).count();
            let up_to_date = results.iter().filter(|r| r.status == PushOutcome::UpToDate).count();
            let failed = results.iter().filter(|r| r.status == PushOutcome::Failed).count();
            println!();
            println!(
                "{created} created, {pushed} pushed, {up_to_date} up-to-date. \
                 {failed} failed."
            );
        }
    }
    let any_failed = results.iter().any(|r| r.status == PushOutcome::Failed);
    Ok(if any_failed { CliResult::GenericFailure } else { CliResult::Success })
}
