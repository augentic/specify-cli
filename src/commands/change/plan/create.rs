use std::io::Write;

use serde::Serialize;
use serde_json::Value;
use specify_domain::change::{Entry, EntryPatch, Patch, Plan, Status};
use specify_domain::config::{InitPolicy, with_state};
use specify_error::Result;

use super::{Ref, change_entry_json, check_project, plan_ref};
use crate::context::Ctx;

/// Convert a CLI-supplied optional string to a [`Patch<String>`]: an
/// absent flag leaves the field unchanged, an empty value clears it,
/// any other value replaces it.
fn cli_patch(value: Option<String>) -> Patch<String> {
    match value {
        None => Patch::Keep,
        Some(s) if s.is_empty() => Patch::Clear,
        Some(s) => Patch::Set(s),
    }
}

pub(super) fn add(
    ctx: &Ctx, name: String, depends_on: Vec<String>, sources: Vec<String>,
    description: Option<String>, project: Option<String>, capability: Option<String>,
    context: Vec<String>,
) -> Result<()> {
    if let Some(proj) = &project {
        check_project(&ctx.project_dir, proj)?;
    }

    let entry = Entry {
        name,
        project,
        capability,
        status: Status::Pending,
        depends_on,
        sources,
        context,
        description,
        status_reason: None,
    };
    let plan_path = ctx.layout().plan_path();
    let body = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| {
            plan.create(entry)?;
            let created =
                plan.entries.last().expect("Plan::create appended an entry that is now missing");
            Ok(EntryBody {
                plan: plan_ref(plan, &plan_path),
                action: PlanAction::Create,
                entry: change_entry_json(created),
            })
        },
    )?;

    ctx.write(&body, write_entry_text)?;
    Ok(())
}

pub(super) fn amend(
    ctx: &Ctx, name: String, depends_on: Option<Vec<String>>, sources: Option<Vec<String>>,
    description: Option<String>, project: Option<String>, capability: Option<String>,
    context: Option<Vec<String>>,
) -> Result<()> {
    if let Some(proj) = &project
        && !proj.is_empty()
    {
        check_project(&ctx.project_dir, proj)?;
    }

    let patch = EntryPatch {
        depends_on,
        sources,
        project: cli_patch(project),
        capability: cli_patch(capability),
        description: cli_patch(description),
        context,
    };
    let plan_path = ctx.layout().plan_path();
    let body = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| {
            plan.amend(&name, patch)?;
            let amended =
                plan.entries.iter().find(|c| c.name == name).expect("amended entry present");
            Ok(EntryBody {
                plan: plan_ref(plan, &plan_path),
                action: PlanAction::Amend,
                entry: change_entry_json(amended),
            })
        },
    )?;

    ctx.write(&body, write_entry_text)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryBody {
    plan: Ref,
    action: PlanAction,
    entry: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum PlanAction {
    Create,
    Amend,
}

fn write_entry_text(w: &mut dyn Write, body: &EntryBody) -> std::io::Result<()> {
    let name = body.entry.get("name").and_then(Value::as_str).unwrap_or("");
    match body.action {
        PlanAction::Create => writeln!(w, "Created plan entry '{name}' with status 'pending'."),
        PlanAction::Amend => writeln!(w, "Amended plan entry '{name}'."),
    }
}
