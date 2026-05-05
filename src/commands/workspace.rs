#![allow(clippy::needless_pass_by_value, clippy::option_if_let_else)]

use serde::Serialize;
use serde_json::Value;
use specify::Error;
use specify_change::Plan;
use specify_registry::Registry;
use specify_registry::merge::{
    MergeProjectResult, MergeStatus, RealGhClient, run_workspace_merge_impl,
};
use specify_registry::workspace::{
    PushOutcome, SlotKind, SlotStatus, run_workspace_push_impl, sync_registry_workspace,
    workspace_status,
};

use super::change::plan::require_file;
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
                        message: "no registry declared at registry.yaml; nothing to sync",
                    });
                }
                OutputFormat::Text => {
                    println!("no registry declared at registry.yaml; nothing to sync");
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
                    println!("no registry declared at registry.yaml");
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
            "No active plan found at plan.yaml. Run 'specify change plan create' \
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

    let results = run_workspace_push_impl(
        &ctx.project_dir,
        &plan.name,
        &registry,
        &projects,
        dry_run,
    )?;

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

// ---------------------------------------------------------------------------
// workspace merge (RFC-9 §4A)
// ---------------------------------------------------------------------------

pub fn run_workspace_merge(
    ctx: &CommandContext, projects: Vec<String>, dry_run: bool,
) -> Result<CliResult, Error> {
    let plan_path = require_file(&ctx.project_dir).map_err(|_err| {
        Error::Config(
            "No active plan found at plan.yaml. Run 'specify change plan create' \
             to author one (or 'specify change create' first if the change brief \
             is also missing) before invoking 'specify workspace merge'."
                .to_string(),
        )
    })?;
    let plan = Plan::load(&plan_path)?;

    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::Config(
            "No registry.yaml found; workspace merge requires a registry. \
             Add projects via `specify registry add`."
                .to_string(),
        ));
    };

    let gh = RealGhClient;
    let results = run_workspace_merge_impl(
        &ctx.project_dir,
        &plan.name,
        &registry,
        &gh,
        &projects,
        dry_run,
    )?;

    let initiative_name = plan.name.clone();
    let expected_branch = format!("specify/{initiative_name}");

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct MergeBody {
                initiative: String,
                expected_branch: String,
                projects: Vec<MergeItem>,
                summary: MergeSummaryCounts,
                #[serde(skip_serializing_if = "Option::is_none")]
                dry_run: Option<bool>,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct MergeItem {
                name: String,
                status: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                pr_number: Option<u64>,
                #[serde(skip_serializing_if = "Option::is_none")]
                url: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                head_ref_name: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                detail: Option<String>,
            }
            let summary = summarise(&results);
            let items: Vec<MergeItem> = results
                .iter()
                .map(|r| MergeItem {
                    name: r.name.clone(),
                    status: r.status.to_string(),
                    pr_number: r.pr_number,
                    url: r.url.clone(),
                    head_ref_name: r.head_ref_name.clone(),
                    detail: r.detail.clone(),
                })
                .collect();
            emit_response(MergeBody {
                initiative: initiative_name,
                expected_branch,
                projects: items,
                summary,
                dry_run: dry_run.then_some(true),
            });
        }
        OutputFormat::Text => {
            print_merge_text(&results, &expected_branch, &plan.name, dry_run);
        }
    }

    let any_failed = results.iter().any(is_failure_status);
    Ok(if any_failed { CliResult::GenericFailure } else { CliResult::Success })
}

/// Statuses that warrant exit 1 (operator action required). `merged`,
/// `would-merge`, and `no-branch` are normal classifications and exit
/// 0; the failure-bucket statuses below force a non-zero exit so CI
/// loops and the 2C umbrella skill can branch on the exit code.
const fn is_failure_status(r: &MergeProjectResult) -> bool {
    matches!(
        r.status,
        MergeStatus::Failed
            | MergeStatus::FailedChecks
            | MergeStatus::PendingChecks
            | MergeStatus::BranchPatternMismatch
            | MergeStatus::Closed
    )
}

fn summarise(results: &[MergeProjectResult]) -> MergeSummaryCounts {
    let mut s = MergeSummaryCounts::default();
    for r in results {
        match r.status {
            MergeStatus::Merged => s.merged += 1,
            MergeStatus::WouldMerge => s.would_merge += 1,
            MergeStatus::PendingChecks => s.pending_checks += 1,
            MergeStatus::FailedChecks => s.failed_checks += 1,
            MergeStatus::Closed => s.closed += 1,
            MergeStatus::NoBranch => s.no_branch += 1,
            MergeStatus::BranchPatternMismatch => s.branch_pattern_mismatch += 1,
            MergeStatus::Failed => s.failed += 1,
        }
    }
    s
}

#[derive(Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct MergeSummaryCounts {
    merged: usize,
    would_merge: usize,
    pending_checks: usize,
    failed_checks: usize,
    closed: usize,
    no_branch: usize,
    branch_pattern_mismatch: usize,
    failed: usize,
}

fn print_merge_text(
    results: &[MergeProjectResult], expected_branch: &str, initiative: &str, dry_run: bool,
) {
    if dry_run {
        println!("[dry-run] specify: workspace merge — {initiative} ({expected_branch})");
    } else {
        println!("specify: workspace merge — {initiative} ({expected_branch})");
    }
    println!();
    for r in results {
        let url = r.url.as_deref().unwrap_or("");
        let pr = r.pr_number.map(|n| format!("PR #{n}")).unwrap_or_default();
        println!("  {:<20} {:<24} {:<10} {}", r.name, r.status, pr, url);
        if let Some(detail) = &r.detail {
            println!("    {detail}");
        }
    }
    let s = summarise(results);
    println!();
    println!(
        "{} merged, {} would-merge, {} pending-checks, {} failed-checks, \
         {} closed, {} no-branch, {} branch-pattern-mismatch, {} failed.",
        s.merged,
        s.would_merge,
        s.pending_checks,
        s.failed_checks,
        s.closed,
        s.no_branch,
        s.branch_pattern_mismatch,
        s.failed,
    );
}
