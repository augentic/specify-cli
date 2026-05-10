#![allow(
    clippy::option_if_let_else,
    reason = "Inline DTOs and chained Option flows keep the workspace dispatcher readable."
)]

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use specify::config::ProjectConfig;
use specify_change::Plan;
use specify_error::Error;
use specify_registry::Registry;
use specify_registry::branch::{
    self, Diagnostic as BranchDiagnostic, Prepared, Request as BranchRequest, prepare,
};
use specify_registry::workspace::{
    ConfiguredTargetKind, PushOutcome, SlotKind, SlotStatus, push_projects,
    status_projects as workspace_status_projects, sync_projects as workspace_sync_projects,
};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn sync(ctx: &CommandContext, projects: Vec<String>) -> Result<CliResult, Error> {
    match Registry::load(&ctx.project_dir)? {
        None => {
            if !projects.is_empty() {
                return Err(Error::Diag {
                    code: "workspace-no-registry",
                    detail:
                        "No registry.yaml found; workspace sync cannot resolve project selectors"
                            .to_string(),
                });
            }
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
                    })?;
                }
                OutputFormat::Text => {
                    println!("no registry declared at registry.yaml; nothing to sync");
                }
            }
            Ok(CliResult::Success)
        }
        Some(registry) => {
            let selected = registry.select(&projects)?;
            workspace_sync_projects(&ctx.project_dir, &selected)?;
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
                    })?;
                }
                OutputFormat::Text => println!("workspace sync complete"),
            }
            Ok(CliResult::Success)
        }
    }
}

pub fn status(ctx: &CommandContext, projects: Vec<String>) -> Result<CliResult, Error> {
    match Registry::load(&ctx.project_dir)? {
        None => {
            if !projects.is_empty() {
                return Err(Error::Diag {
                    code: "workspace-no-registry",
                    detail:
                        "No registry.yaml found; workspace status cannot resolve project selectors"
                            .to_string(),
                });
            }
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
                    })?;
                }
                OutputFormat::Text => {
                    println!("no registry declared at registry.yaml");
                }
            }
            Ok(CliResult::Success)
        }
        Some(registry) => {
            let selected = registry.select(&projects)?;
            let slots = workspace_status_projects(&ctx.project_dir, &selected);
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct StatusBody {
                        slots: Vec<Value>,
                    }
                    let items: Vec<Value> = slots.iter().map(slot_to_json).collect();
                    emit_response(StatusBody { slots: items })?;
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

pub fn prepare_branch(
    ctx: &CommandContext, project: String, change: String, sources: Vec<PathBuf>,
    outputs: Vec<PathBuf>,
) -> Result<CliResult, Error> {
    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::Diag {
            code: "workspace-no-registry",
            detail: "No registry.yaml found; workspace prepare-branch requires a registry"
                .to_string(),
        });
    };
    let selected = registry.select(std::slice::from_ref(&project))?;
    let Some(project) = selected.first() else {
        return Err(Error::Diag {
            code: "workspace-prepare-branch-no-project",
            detail: "workspace prepare-branch resolved no project".to_string(),
        });
    };
    let request = BranchRequest {
        change_name: change,
        source_paths: sources,
        output_paths: outputs,
    };

    match prepare(&ctx.project_dir, project, &request) {
        Ok(prepared) => {
            render_branch_preparation_success(ctx.format, &prepared)?;
            Ok(CliResult::Success)
        }
        Err(diagnostic) => {
            render_branch_preparation_failure(ctx.format, &diagnostic)?;
            Ok(CliResult::GenericFailure)
        }
    }
}

fn render_branch_preparation_success(
    format: OutputFormat, prepared: &Prepared,
) -> Result<(), Error> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PrepareBranchBody<'a> {
                prepared: bool,
                project: &'a str,
                branch: &'a str,
                slot_path: &'a str,
                base_ref: &'a str,
                base_sha: &'a str,
                local_branch: &'a branch::LocalAction,
                remote_branch: &'a branch::RemoteAction,
                dirty: &'a branch::Dirty,
                diagnostics: Vec<BranchDiagnostic>,
            }
            emit_response(PrepareBranchBody {
                prepared: true,
                project: &prepared.project,
                branch: &prepared.branch,
                slot_path: &prepared.slot_path,
                base_ref: &prepared.base_ref,
                base_sha: &prepared.base_sha,
                local_branch: &prepared.local_branch,
                remote_branch: &prepared.remote_branch,
                dirty: &prepared.dirty,
                diagnostics: Vec::new(),
            })?;
        }
        OutputFormat::Text => {
            println!(
                "workspace branch prepared: {} {} ({:?}, {:?})",
                prepared.project, prepared.branch, prepared.local_branch, prepared.remote_branch
            );
            if !prepared.dirty.tracked_allowed.is_empty() || !prepared.dirty.untracked.is_empty() {
                println!(
                    "dirty: {} tracked resume-safe, {} untracked",
                    prepared.dirty.tracked_allowed.len(),
                    prepared.dirty.untracked.len()
                );
            }
        }
    }
    Ok(())
}

fn render_branch_preparation_failure(
    format: OutputFormat, diagnostic: &BranchDiagnostic,
) -> Result<(), Error> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PrepareBranchFailure<'a> {
                error: &'static str,
                exit_code: u8,
                diagnostic: &'a BranchDiagnostic,
            }
            emit_response(PrepareBranchFailure {
                error: "branch-preparation-failed",
                exit_code: CliResult::GenericFailure.code(),
                diagnostic,
            })?;
        }
        OutputFormat::Text => {
            eprintln!("error: {}", diagnostic.message);
            eprintln!("diagnostic: {}", diagnostic.key);
            if !diagnostic.paths.is_empty() {
                eprintln!("paths: {}", diagnostic.paths.join(", "));
            }
        }
    }
    Ok(())
}

const fn kind_label(kind: SlotKind) -> &'static str {
    match kind {
        SlotKind::Missing => "missing",
        SlotKind::Symlink => "symlink",
        SlotKind::GitClone => "git-clone",
        SlotKind::Other => "other",
    }
}

const fn configured_target_kind_label(kind: ConfiguredTargetKind) -> &'static str {
    match kind {
        ConfiguredTargetKind::Local => "local",
        ConfiguredTargetKind::Remote => "remote",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SlotJson {
    name: String,
    kind: &'static str,
    slot_path: String,
    configured_target_kind: &'static str,
    configured_target: String,
    actual_symlink_target: Option<String>,
    actual_origin: Option<String>,
    current_branch: Option<String>,
    head_sha: Option<String>,
    dirty: Option<bool>,
    branch_matches_change: Option<bool>,
    project_config_present: bool,
    active_slices: Vec<String>,
}

fn slot_to_json(slot: &SlotStatus) -> Value {
    serde_json::to_value(SlotJson {
        name: slot.name.clone(),
        kind: kind_label(slot.kind),
        slot_path: slot.slot_path.display().to_string(),
        configured_target_kind: configured_target_kind_label(slot.configured_target_kind),
        configured_target: slot.configured_target.clone(),
        actual_symlink_target: slot
            .actual_symlink_target
            .as_ref()
            .map(|path| path.display().to_string()),
        actual_origin: slot.actual_origin.clone(),
        current_branch: slot.current_branch.clone(),
        head_sha: slot.head_sha.clone(),
        dirty: slot.dirty,
        branch_matches_change: slot.branch_matches_change,
        project_config_present: slot.project_config_present,
        active_slices: slot.active_slices.clone(),
    })
    .expect("SlotJson serialises")
}

fn print_slot(slot: &SlotStatus) {
    let kind = kind_label(slot.kind);
    let head = slot.head_sha.as_deref().unwrap_or("-");
    let origin = slot.actual_origin.as_deref().unwrap_or("-");
    let branch = slot.current_branch.as_deref().unwrap_or("-");
    let symlink_target = slot
        .actual_symlink_target
        .as_ref()
        .map_or_else(|| "-".to_string(), |path| path.display().to_string());
    let dirty = match slot.dirty {
        None => "-",
        Some(true) => "yes",
        Some(false) => "no",
    };
    let change_branch = match slot.branch_matches_change {
        None => "-",
        Some(true) => "match",
        Some(false) => "mismatch",
    };
    let project_config = if slot.project_config_present { "present" } else { "missing" };
    let slices =
        if slot.active_slices.is_empty() { "-".to_string() } else { slot.active_slices.join(",") };
    println!(
        "{}: kind={kind} path={} configured-{}={} target={} origin={} branch={} change-branch={} head={} dirty={} project.yaml={} active-slices={}",
        slot.name,
        slot.slot_path.display(),
        configured_target_kind_label(slot.configured_target_kind),
        slot.configured_target,
        symlink_target,
        origin,
        branch,
        change_branch,
        head,
        dirty,
        project_config,
        slices
    );
}

pub fn push(
    ctx: &CommandContext, projects: Vec<String>, dry_run: bool,
) -> Result<CliResult, Error> {
    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::Diag {
            code: "workspace-no-registry",
            detail: "No registry.yaml found; workspace push requires a registry".to_string(),
        });
    };
    let selected = registry.select(&projects)?;

    let plan_path = ProjectConfig::plan_path(&ctx.project_dir);
    if !plan_path.exists() {
        return Err(Error::Diag {
            code: "workspace-push-no-plan",
            detail: "No active plan found at plan.yaml. Run 'specify change plan create' \
                     to create one, or check whether the plan was already archived."
                .to_string(),
        });
    }
    let plan = Plan::load(&plan_path)?;

    let results = push_projects(&ctx.project_dir, &plan.name, &selected, dry_run)?;

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
                #[serde(skip_serializing_if = "Option::is_none")]
                error: Option<String>,
            }
            let items: Vec<PushItem> = results
                .iter()
                .map(|r| PushItem {
                    name: r.name.clone(),
                    status: r.status.to_string(),
                    branch: r.branch.clone(),
                    pr: r.pr_number,
                    error: r.error.clone(),
                })
                .collect();
            emit_response(PushBody {
                projects: items,
                dry_run: dry_run.then_some(true),
            })?;
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
            let local_only = results.iter().filter(|r| r.status == PushOutcome::LocalOnly).count();
            let no_branch = results.iter().filter(|r| r.status == PushOutcome::NoBranch).count();
            let failed = results.iter().filter(|r| r.status == PushOutcome::Failed).count();
            println!();
            println!(
                "{created} created, {pushed} pushed, {up_to_date} up-to-date, \
                 {local_only} local-only, {no_branch} no-branch. \
                 {failed} failed."
            );
        }
    }
    let any_failed = results.iter().any(|r| r.status == PushOutcome::Failed);
    Ok(if any_failed { CliResult::GenericFailure } else { CliResult::Success })
}
