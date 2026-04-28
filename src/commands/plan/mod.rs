#![allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]

mod create;
mod doctor;
mod lifecycle;
mod lock;
mod status;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify::{
    Error, Plan, PlanChange, PlanValidationLevel, PlanValidationResult, ProjectConfig, Registry,
};

use crate::cli::{LockAction, OutputFormat, PlanAction};
use crate::context::CommandContext;
pub(super) use crate::output::absolute_string;
use crate::output::{CliResult, emit_response};

pub fn run_plan(ctx: &CommandContext, action: PlanAction) -> Result<CliResult, Error> {
    match action {
        PlanAction::Create { name, sources } => lifecycle::run_plan_create(ctx, name, sources),
        PlanAction::Validate => lifecycle::run_plan_validate(ctx),
        PlanAction::Doctor => doctor::run_plan_doctor(ctx),
        PlanAction::Next => lifecycle::run_plan_next(ctx),
        PlanAction::Status => status::run_plan_status(ctx),
        PlanAction::Add {
            name,
            depends_on,
            sources,
            description,
            project,
            schema,
            context,
        } => create::run_plan_add(
            ctx,
            name,
            depends_on,
            sources,
            description,
            project,
            schema,
            context,
        ),
        PlanAction::Amend {
            name,
            depends_on,
            sources,
            description,
            project,
            schema,
            context,
        } => create::run_plan_amend(
            ctx,
            name,
            depends_on,
            sources,
            description,
            project,
            schema,
            context,
        ),
        PlanAction::Transition { name, target, reason } => {
            lifecycle::run_plan_transition(ctx, name, target, reason)
        }
        PlanAction::Archive { force } => lifecycle::run_plan_archive(ctx, force),
        PlanAction::Lock { action } => match action {
            LockAction::Acquire { pid } => lock::run_plan_lock_acquire(ctx, pid),
            LockAction::Release { pid } => lock::run_plan_lock_release(ctx, pid),
            LockAction::Status => lock::run_plan_lock_status(ctx),
        },
    }
}

// ---- Shared helpers used across submodules ----

/// `<project_dir>/.specify/plan.yaml`.
pub fn file_path(project_dir: &Path) -> PathBuf {
    ProjectConfig::specify_dir(project_dir).join("plan.yaml")
}

/// Ensure the plan file exists before we try to load it. Error text is
/// the stable "plan file not found: .specify/plan.yaml" string that
/// skill authors match on.
pub fn require_file(project_dir: &Path) -> Result<PathBuf, Error> {
    let path = file_path(project_dir);
    if !path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path,
        });
    }
    Ok(path)
}

pub(super) const fn level_label(level: &PlanValidationLevel) -> &'static str {
    match level {
        PlanValidationLevel::Error => "error",
        PlanValidationLevel::Warning => "warning",
    }
}

/// Emit the stable "go run `specify plan validate`" pointer when
/// `plan next` or `plan status` is asked to operate on a
/// structurally broken plan.
pub(super) fn emit_structural_error(format: OutputFormat) -> CliResult {
    let msg = "plan has structural errors; run 'specify plan validate' for detail";
    match format {
        OutputFormat::Json => emit_response(crate::output::ErrorResponse {
            error: "validation".to_string(),
            message: msg.to_string(),
            exit_code: CliResult::ValidationFailed.code(),
        }),
        OutputFormat::Text => eprintln!("error: {msg}"),
    }
    CliResult::ValidationFailed
}

pub(super) fn load_for_write(ctx: &CommandContext) -> Result<(PathBuf, Plan), Error> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    Ok((plan_path, plan))
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct PlanRef {
    pub name: String,
    pub path: String,
}

pub(super) fn plan_ref(plan: &Plan, plan_path: &Path) -> PlanRef {
    PlanRef {
        name: plan.name.clone(),
        path: plan_path.display().to_string(),
    }
}

/// Serialize a `PlanChange` into the on-the-wire kebab-case JSON shape.
pub(super) fn change_entry_json(entry: &PlanChange) -> Value {
    serde_json::to_value(entry).expect("PlanChange serialises as JSON")
}

/// Verify that `project_name` appears in `.specify/registry.yaml`.
pub(super) fn check_project(project_dir: &Path, project_name: &str) -> Result<(), Error> {
    match Registry::load(project_dir) {
        Ok(Some(registry)) => {
            if !registry.projects.iter().any(|p| p.name == project_name) {
                return Err(Error::Config(format!(
                    "--project '{project_name}' does not match any project in registry.yaml"
                )));
            }
            Ok(())
        }
        Ok(None) => {
            Err(Error::Config("--project was specified but no registry.yaml exists".to_string()))
        }
        Err(err) => Err(err),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ValidationRow<'a> {
    level: &'a str,
    code: &'a str,
    entry: &'a Option<String>,
    message: &'a str,
}

pub(super) fn validation_to_json(r: &PlanValidationResult) -> Value {
    serde_json::to_value(ValidationRow {
        level: level_label(&r.level),
        code: r.code,
        entry: &r.entry,
        message: &r.message,
    })
    .expect("ValidationRow serialises")
}

pub(super) fn print_validation_line(r: &PlanValidationResult) {
    let level = match r.level {
        PlanValidationLevel::Error => "ERROR  ",
        PlanValidationLevel::Warning => "WARNING",
    };
    let entry_col = r.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
    println!("{level} {:<32} {:<24} {}", r.code, entry_col, r.message);
}
