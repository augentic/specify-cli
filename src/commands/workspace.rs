
use serde::Serialize;
use serde_json::Value;
use specify::{
    Error, Plan, Registry, WorkspaceSlotKind, WorkspaceSlotStatus,
    sync_registry_workspace, workspace_status,
};

use crate::cli::OutputFormat;
use crate::output::{CliResult, emit_error, emit_response};

use super::plan::require_plan_file;
use super::require_project;

pub(crate) fn run_initiative_workspace_sync(format: OutputFormat) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    match Registry::load(&project_dir) {
        Ok(None) => {
            match format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct WorkspaceSyncAbsentResponse {
                        registry: Value,
                        synced: bool,
                        message: &'static str,
                    }
                    emit_response(WorkspaceSyncAbsentResponse {
                        registry: Value::Null,
                        synced: false,
                        message: "no registry declared at .specify/registry.yaml; nothing to sync",
                    })
                },
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml; nothing to sync");
                }
            }
            CliResult::Success
        }
        Ok(Some(registry)) => {
            if let Err(err) = sync_registry_workspace(&project_dir) {
                return emit_error(format, &err);
            }
            match format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct WorkspaceSyncResponse {
                        registry: Registry,
                        synced: bool,
                    }
                    emit_response(WorkspaceSyncResponse {
                        registry,
                        synced: true,
                    })
                },
                OutputFormat::Text => println!("workspace sync complete"),
            }
            CliResult::Success
        }
        Err(err) => emit_error(format, &err),
    }
}

pub(crate) fn run_initiative_workspace_status(format: OutputFormat) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    match workspace_status(&project_dir) {
        Ok(None) => {
            match format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct WorkspaceStatusAbsentResponse {
                        registry: Value,
                        slots: Value,
                    }
                    emit_response(WorkspaceStatusAbsentResponse {
                        registry: Value::Null,
                        slots: Value::Null,
                    })
                },
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml");
                }
            }
            CliResult::Success
        }
        Ok(Some(slots)) => {
            match format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct WorkspaceStatusResponse {
                        slots: Vec<Value>,
                    }
                    let items: Vec<Value> = slots.iter().map(workspace_slot_to_json).collect();
                    emit_response(WorkspaceStatusResponse {
                        slots: items,
                    });
                }
                OutputFormat::Text => {
                    for slot in &slots {
                        print_workspace_slot_line(slot);
                    }
                }
            }
            CliResult::Success
        }
        Err(err) => emit_error(format, &err),
    }
}

fn workspace_slot_kind_label(kind: WorkspaceSlotKind) -> &'static str {
    match kind {
        WorkspaceSlotKind::Missing => "missing",
        WorkspaceSlotKind::Symlink => "symlink",
        WorkspaceSlotKind::GitClone => "git-clone",
        WorkspaceSlotKind::Other => "other",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct WorkspaceSlotJson {
    name: String,
    kind: &'static str,
    head_sha: Option<String>,
    dirty: Option<bool>,
}

fn workspace_slot_to_json(slot: &WorkspaceSlotStatus) -> Value {
    serde_json::to_value(WorkspaceSlotJson {
        name: slot.name.clone(),
        kind: workspace_slot_kind_label(slot.kind),
        head_sha: slot.head_sha.clone(),
        dirty: slot.dirty,
    }).expect("WorkspaceSlotJson serialises")
}

fn print_workspace_slot_line(slot: &WorkspaceSlotStatus) {
    let kind = workspace_slot_kind_label(slot.kind);
    let head = slot.head_sha.as_deref().unwrap_or("-");
    let dirty = match slot.dirty {
        None => "-",
        Some(true) => "yes",
        Some(false) => "no",
    };
    println!("{}: kind={kind} head={head} dirty={dirty}", slot.name);
}

pub(crate) fn run_workspace_push(format: OutputFormat, projects: Vec<String>, dry_run: bool) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    let Ok(plan_path) = require_plan_file(&project_dir) else {
        let err = Error::Config(
            "No active plan found at .specify/plan.yaml. Run 'specify plan init' \
             to create one, or check whether the plan was already archived."
                .to_string(),
        );
        return emit_error(format, &err);
    };
    let plan = match Plan::load(&plan_path) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };

    let registry = match Registry::load(&project_dir) {
        Ok(Some(r)) => r,
        Ok(None) => {
            let err = Error::Config(
                "No registry.yaml found; workspace push requires a registry".to_string(),
            );
            return emit_error(format, &err);
        }
        Err(err) => return emit_error(format, &err),
    };

    match specify::run_workspace_push_impl(&project_dir, &plan, &registry, &projects, dry_run) {
        Ok(results) => {
            match format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct WorkspacePushResponse {
                        projects: Vec<WorkspacePushItem>,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        dry_run: Option<bool>,
                    }
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct WorkspacePushItem {
                        name: String,
                        status: String,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        branch: Option<String>,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        pr: Option<u64>,
                    }
                    let items: Vec<WorkspacePushItem> = results
                        .iter()
                        .map(|r| WorkspacePushItem {
                            name: r.name.clone(),
                            status: r.status.clone(),
                            branch: r.branch.clone(),
                            pr: r.pr_number,
                        })
                        .collect();
                    emit_response(WorkspacePushResponse {
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
                            if dry_run && (r.status == "pushed" || r.status == "created") {
                                format!("would-{}", r.status)
                            } else {
                                r.status.clone()
                            };
                        let branch_part = r.branch.as_deref().unwrap_or("");
                        let pr_part = r.pr_number.map(|n| format!("PR #{n}")).unwrap_or_default();
                        println!(
                            "  {:<20} {:<14} {} {}",
                            r.name, status_label, branch_part, pr_part
                        );
                    }
                    let created = results.iter().filter(|r| r.status == "created").count();
                    let pushed = results.iter().filter(|r| r.status == "pushed").count();
                    let up_to_date = results.iter().filter(|r| r.status == "up-to-date").count();
                    let failed = results.iter().filter(|r| r.status == "failed").count();
                    println!();
                    println!(
                        "{created} created, {pushed} pushed, {up_to_date} up-to-date. \
                         {failed} failed."
                    );
                }
            }
            let any_failed = results.iter().any(|r| r.status == "failed");
            if any_failed { CliResult::GenericFailure } else { CliResult::Success }
        }
        Err(err) => emit_error(format, &err),
    }
}

