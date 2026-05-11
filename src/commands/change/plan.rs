#![allow(
    clippy::too_many_arguments,
    reason = "Plan dispatcher passes through clap-shaped argument tuples."
)]

pub mod cli;
mod create;
mod doctor;
mod lifecycle;
mod lock;
mod status;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify_change::{Entry, Plan};
use specify_config::ProjectConfig;
use specify_error::{Error, Result};
use specify_registry::Registry;

use crate::cli::{LockAction, PlanAction};
use crate::context::CommandContext;
use crate::output::CliResult;
pub(super) use crate::output::path_string;

pub fn run(ctx: &CommandContext, action: PlanAction) -> Result<CliResult> {
    // Most arms emit unconditionally and return `Result<()>`; only
    // `validate`, `doctor`, and `archive` surface non-success exits
    // (`ValidationFailed` / `GenericFailure`). Lift the rest into
    // `CliResult::Success` here so each sub-handler can focus on its
    // payload.
    let ok = |()| CliResult::Success;
    match action {
        PlanAction::Create { name, sources } => lifecycle::create(ctx, name, sources).map(ok),
        PlanAction::Validate => lifecycle::validate(ctx),
        PlanAction::Doctor => doctor::run(ctx),
        PlanAction::Next => lifecycle::next(ctx).map(ok),
        PlanAction::Status => status::run(ctx).map(ok),
        PlanAction::Add {
            name,
            depends_on,
            sources,
            description,
            project,
            capability,
            context,
        } => create::add(ctx, name, depends_on, sources, description, project, capability, context)
            .map(ok),
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
                .map(ok)
        }
        PlanAction::Transition { name, target, reason } => {
            lifecycle::transition(ctx, name, target, reason).map(ok)
        }
        PlanAction::Archive { force } => lifecycle::archive(ctx, force),
        PlanAction::Lock { action } => match action {
            LockAction::Acquire { pid } => lock::acquire(ctx, pid).map(ok),
            LockAction::Release { pid } => lock::release(ctx, pid).map(ok),
            LockAction::Status => lock::status(ctx).map(ok),
        },
    }
}

// ---- Shared helpers used across submodules ----

/// Ensure the plan file exists before we try to load it. Error text is
/// the stable "plan file not found: plan.yaml" string that skill
/// authors match on.
pub fn require_file(project_dir: &Path) -> Result<PathBuf> {
    let path = ProjectConfig::plan_path(project_dir);
    if !path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path,
        });
    }
    Ok(path)
}

pub(super) fn load_for_write(ctx: &CommandContext) -> Result<(PathBuf, Plan)> {
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
pub(super) fn check_project(project_dir: &Path, project_name: &str) -> Result<()> {
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
