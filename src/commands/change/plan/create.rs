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
        None => Patch::keep(),
        Some(s) if s.is_empty() => Patch::clear(),
        Some(s) => Patch::set(s),
    }
}

pub(super) fn add(
    ctx: &Ctx, name: String, depends_on: Vec<String>, sources: Vec<String>,
    description: Option<String>, project: Option<String>, capability: Option<String>,
    context: Vec<String>,
) -> Result<()> {
    if let Some(ref proj) = project {
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
            Ok(AddBody {
                plan: plan_ref(plan, &plan_path),
                action: PlanAction::Create,
                entry: change_entry_json(created),
            })
        },
    )?;

    ctx.write(&body, write_add_text)?;
    Ok(())
}

pub(super) fn amend(
    ctx: &Ctx, name: String, depends_on: Option<Vec<String>>, sources: Option<Vec<String>>,
    description: Option<String>, project: Option<String>, capability: Option<String>,
    context: Option<Vec<String>>,
) -> Result<()> {
    if let Some(ref proj) = project
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
            Ok(AmendBody {
                plan: plan_ref(plan, &plan_path),
                action: PlanAction::Amend,
                entry: change_entry_json(amended),
            })
        },
    )?;

    ctx.write(&body, write_amend_text)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AddBody {
    plan: Ref,
    action: PlanAction,
    entry: Value,
}

fn write_add_text(w: &mut dyn Write, body: &AddBody) -> std::io::Result<()> {
    let name = body.entry.get("name").and_then(Value::as_str).unwrap_or("");
    writeln!(w, "Created plan entry '{name}' with status 'pending'.")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AmendBody {
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

fn write_amend_text(w: &mut dyn Write, body: &AmendBody) -> std::io::Result<()> {
    let name = body.entry.get("name").and_then(Value::as_str).unwrap_or("");
    writeln!(w, "Amended plan entry '{name}'.")
}
