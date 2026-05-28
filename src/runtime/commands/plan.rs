#![allow(
    clippy::too_many_arguments,
    reason = "Plan dispatcher passes through clap-shaped argument tuples."
)]

mod add;
mod amend;
mod args;
pub mod cli;
mod create;
mod entry;
mod lifecycle;

use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_domain::change::Plan;
use specify_domain::config::Layout;
use specify_domain::registry::Registry;
use specify_error::{Error, Result};

use self::cli::PlanAction;
use crate::runtime::context::Ctx;

pub fn run(ctx: &Ctx, action: PlanAction) -> Result<()> {
    match action {
        PlanAction::Create {
            name,
            sources,
            divergence_likely,
            auto_approve,
            authority_override,
        } => create::create(
            ctx,
            name,
            sources,
            &divergence_likely,
            auto_approve,
            &authority_override,
        ),
        PlanAction::Validate => lifecycle::validate(ctx),
        PlanAction::Next => lifecycle::next(ctx),
        PlanAction::Add {
            name,
            depends_on,
            sources,
            description,
            project,
            target,
            context,
            authority_override,
        } => add::add(
            ctx,
            &name,
            depends_on,
            sources,
            description,
            project,
            target,
            context,
            &authority_override,
        ),
        PlanAction::Amend {
            name,
            depends_on,
            sources,
            add_source,
            remove_source,
            divergence,
            description,
            project,
            target,
            context,
            authority_override,
            clear_authority_override,
            clear_authority_overrides,
            add_alias,
            remove_alias,
        } => amend::amend(
            ctx,
            name,
            depends_on,
            sources,
            add_source,
            remove_source,
            divergence.as_deref(),
            description,
            project,
            target,
            context,
            &authority_override,
            &clear_authority_override,
            &clear_authority_overrides,
            &add_alias,
            &remove_alias,
        ),
        PlanAction::Transition { name, target, undo } => {
            lifecycle::transition(ctx, name, target, undo)
        }
        PlanAction::Archive { force } => lifecycle::archive(ctx, force),
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
