#![allow(clippy::items_after_statements, clippy::too_many_arguments)]

use serde::Serialize;
use serde_json::Value;
use specify::{Error, PlanChange, PlanChangePatch, PlanStatus};

use super::{
    PlanRef, load_plan_for_write, plan_change_entry_json, plan_ref_from,
    validate_project_in_registry,
};
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_plan_create(
    ctx: &CommandContext, name: String, depends_on: Vec<String>, sources: Vec<String>,
    description: Option<String>, project: Option<String>, schema: Option<String>,
    context: Vec<String>,
) -> Result<CliResult, Error> {
    let (plan_path, mut plan) = load_plan_for_write(ctx)?;

    if let Some(ref proj) = project {
        validate_project_in_registry(&ctx.project_dir, proj)?;
    }

    let entry = PlanChange {
        name: name.clone(),
        project,
        schema,
        status: PlanStatus::Pending,
        depends_on,
        sources,
        context,
        description,
        status_reason: None,
    };

    plan.create(entry)?;
    plan.save(&plan_path)?;

    let created = plan.changes.last().expect("Plan::create appended an entry that is now missing");

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PlanCreateResponse {
        plan: PlanRef,
        action: &'static str,
        entry: Value,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PlanCreateResponse {
            plan: plan_ref_from(&plan, &plan_path),
            action: "create",
            entry: plan_change_entry_json(created),
        }),
        OutputFormat::Text => {
            println!("Created plan entry '{name}' with status 'pending'.");
        }
    }
    Ok(CliResult::Success)
}

pub fn run_plan_amend(
    ctx: &CommandContext, name: String, depends_on: Option<Vec<String>>,
    sources: Option<Vec<String>>, description: Option<String>, project: Option<String>,
    schema: Option<String>, context: Option<Vec<String>>,
) -> Result<CliResult, Error> {
    let (plan_path, mut plan) = load_plan_for_write(ctx)?;

    if let Some(ref proj) = project
        && !proj.is_empty()
    {
        validate_project_in_registry(&ctx.project_dir, proj)?;
    }

    let description_patch: Option<Option<String>> =
        description.map(|s| if s.is_empty() { None } else { Some(s) });
    let project_patch: Option<Option<String>> =
        project.map(|s| if s.is_empty() { None } else { Some(s) });
    let schema_patch: Option<Option<String>> =
        schema.map(|s| if s.is_empty() { None } else { Some(s) });

    let patch = PlanChangePatch {
        depends_on,
        sources,
        project: project_patch,
        schema: schema_patch,
        description: description_patch,
        context,
    };

    plan.amend(&name, patch)?;
    plan.save(&plan_path)?;

    let amended = plan.changes.iter().find(|c| c.name == name).expect("amended entry present");

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PlanAmendResponse {
        plan: PlanRef,
        action: &'static str,
        entry: Value,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PlanAmendResponse {
            plan: plan_ref_from(&plan, &plan_path),
            action: "amend",
            entry: plan_change_entry_json(amended),
        }),
        OutputFormat::Text => {
            println!("Amended plan entry '{name}'.");
        }
    }
    Ok(CliResult::Success)
}
