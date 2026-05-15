#![allow(
    clippy::too_many_arguments,
    reason = "Plan dispatcher passes through clap-shaped argument tuples."
)]

pub mod cli;
mod create;
mod lifecycle;
mod lock;
mod status;

use std::path::{Path, PathBuf};

pub use create::{build_source_map, require_kebab_change_name, write_scaffold};
use serde::Serialize;
use serde_json::Value;
use specify_domain::change::{Entry, Plan};
use specify_domain::config::Layout;
use specify_domain::registry::Registry;
use specify_error::{Error, Result};

use crate::cli::{LockAction, PlanAction};
use crate::context::Ctx;

pub fn run(ctx: &Ctx, action: PlanAction) -> Result<()> {
    match action {
        PlanAction::Create { name, sources } => create::create(ctx, name, sources),
        PlanAction::Validate => lifecycle::validate(ctx),
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
    let path = Layout::new(project_dir).plan_path();
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
pub(super) struct Ref {
    pub name: String,
    pub path: String,
}

pub(super) fn plan_ref(plan: &Plan, plan_path: &Path) -> Ref {
    Ref {
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
