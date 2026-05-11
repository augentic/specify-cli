#![allow(
    clippy::too_many_arguments,
    reason = "Plan dispatcher passes through clap-shaped argument tuples."
)]

pub(crate) mod cli;
mod create;
mod doctor;
mod lifecycle;
mod lock;
mod status;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify_change::{Entry, Plan};
use specify_config::LayoutExt;
use specify_error::{Error, Result};
use specify_registry::Registry;

use crate::cli::{LockAction, PlanAction};
use crate::context::Ctx;
pub(super) use crate::output::display;

pub(super) fn run(ctx: &Ctx, action: PlanAction) -> Result<()> {
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
pub(super) fn require_file(project_dir: &Path) -> Result<PathBuf> {
    let path = project_dir.layout().plan_path();
    if !path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path,
        });
    }
    Ok(path)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct PlanRef {
    pub name: String,
    #[serde(serialize_with = "crate::output::serialize_path")]
    pub path: PathBuf,
}

pub(super) fn plan_ref(plan: &Plan, plan_path: &Path) -> PlanRef {
    PlanRef {
        name: plan.name.clone(),
        path: plan_path.to_path_buf(),
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
