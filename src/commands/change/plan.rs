#![allow(
    clippy::too_many_arguments,
    reason = "Plan dispatcher passes through clap-shaped argument tuples."
)]

mod create;
mod doctor;
mod lifecycle;
mod lock;
mod status;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify::config::ProjectConfig;
use specify_change::{Entry, Finding, Plan, Severity};
use specify_error::Error;
use specify_registry::Registry;

use crate::cli::{LockAction, OutputFormat, PlanAction};
use crate::context::CommandContext;
pub(super) use crate::output::absolute_string;
use crate::output::{CliResult, emit_response};

pub fn run(ctx: &CommandContext, action: PlanAction) -> Result<CliResult, Error> {
    match action {
        PlanAction::Create { name, sources } => lifecycle::create(ctx, name, sources),
        PlanAction::Validate => lifecycle::validate(ctx),
        PlanAction::Doctor => doctor::run(ctx),
        PlanAction::Next => lifecycle::next(ctx),
        PlanAction::Status => status::run(ctx),
        PlanAction::Add {
            name,
            depends_on,
            sources,
            description,
            project,
            capability,
            context,
        } => create::add(ctx, name, depends_on, sources, description, project, capability, context),
        PlanAction::Amend {
            name,
            depends_on,
            sources,
            description,
            project,
            capability,
            context,
        } => {
            create::amend(ctx, name, depends_on, sources, description, project, capability, context)
        }
        PlanAction::Transition { name, target, reason } => {
            lifecycle::transition(ctx, name, target, reason)
        }
        PlanAction::Archive { force } => lifecycle::archive(ctx, force),
        PlanAction::Lock { action } => match action {
            LockAction::Acquire { pid } => lock::acquire(ctx, pid),
            LockAction::Release { pid } => lock::release(ctx, pid),
            LockAction::Status => lock::status(ctx),
        },
    }
}

// ---- Shared helpers used across submodules ----

/// Ensure the plan file exists before we try to load it. Error text is
/// the stable "plan file not found: plan.yaml" string that skill
/// authors match on.
pub fn require_file(project_dir: &Path) -> Result<PathBuf, Error> {
    let path = ProjectConfig::plan_path(project_dir);
    if !path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path,
        });
    }
    Ok(path)
}

pub(super) const fn level_label(level: &Severity) -> &'static str {
    match level {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

/// Emit the stable "go run `specify change plan validate`" pointer when
/// `change plan next` or `change plan status` is asked to operate on a
/// structurally broken plan.
pub(super) fn emit_structural_error(format: OutputFormat) -> Result<CliResult, Error> {
    let msg = "plan has structural errors; run 'specify change plan validate' for detail";
    match format {
        OutputFormat::Json => emit_response(crate::output::ErrorResponse {
            error: "validation".to_string(),
            message: msg.to_string(),
            exit_code: CliResult::ValidationFailed.code(),
        })?,
        OutputFormat::Text => eprintln!("error: {msg}"),
    }
    Ok(CliResult::ValidationFailed)
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

/// Serialize a plan `Entry` into the on-the-wire kebab-case JSON shape.
pub(super) fn change_entry_json(entry: &Entry) -> Value {
    serde_json::to_value(entry).expect("plan Entry serialises as JSON")
}

/// Verify that `project_name` appears in `registry.yaml`.
pub(super) fn check_project(project_dir: &Path, project_name: &str) -> Result<(), Error> {
    match Registry::load(project_dir) {
        Ok(Some(registry)) => {
            if !registry.projects.iter().any(|p| p.name == project_name) {
                return Err(Error::Diag {
                    code: "plan-project-unknown",
                    detail: format!(
                        "--project '{project_name}' does not match any project in registry.yaml"
                    ),
                });
            }
            Ok(())
        }
        Ok(None) => Err(Error::Diag {
            code: "plan-project-no-registry",
            detail: "--project was specified but no registry.yaml exists".to_string(),
        }),
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

pub(super) fn validation_to_json(r: &Finding) -> Value {
    serde_json::to_value(ValidationRow {
        level: level_label(&r.level),
        code: r.code,
        entry: &r.entry,
        message: &r.message,
    })
    .expect("ValidationRow serialises")
}

pub(super) fn print_validation_line(r: &Finding) {
    let level = match r.level {
        Severity::Error => "ERROR  ",
        Severity::Warning => "WARNING",
    };
    let entry_col = r.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
    println!("{level} {:<32} {:<24} {}", r.code, entry_col, r.message);
}
