use std::io::Write;

use serde::Serialize;
use serde_json::Value;
use specify_change::{Entry, EntryPatch, Plan, Status};
use specify_config::with_existing_state;
use specify_error::Result;

use super::{PlanRef, change_entry_json, check_project, plan_ref};
use crate::context::Ctx;
use crate::output::Render;

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
    let body = with_existing_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
        plan.create(entry)?;
        let created =
            plan.entries.last().expect("Plan::create appended an entry that is now missing");
        Ok(AddBody {
            plan: plan_ref(plan, &plan_path),
            action: PlanAction::Create,
            entry: change_entry_json(created),
        })
    })?;

    ctx.out().write(&body)?;
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

    let description_patch: Option<Option<String>> =
        description.map(|s| if s.is_empty() { None } else { Some(s) });
    let project_patch: Option<Option<String>> =
        project.map(|s| if s.is_empty() { None } else { Some(s) });
    let capability_patch: Option<Option<String>> =
        capability.map(|s| if s.is_empty() { None } else { Some(s) });

    let patch = EntryPatch {
        depends_on,
        sources,
        project: project_patch,
        capability: capability_patch,
        description: description_patch,
        context,
    };
    let plan_path = ctx.layout().plan_path();
    let body = with_existing_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
        plan.amend(&name, patch)?;
        let amended = plan.entries.iter().find(|c| c.name == name).expect("amended entry present");
        Ok(AmendBody {
            plan: plan_ref(plan, &plan_path),
            action: PlanAction::Amend,
            entry: change_entry_json(amended),
        })
    })?;

    ctx.out().write(&body)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AddBody {
    plan: PlanRef,
    action: PlanAction,
    entry: Value,
}

impl Render for AddBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let name = self.entry.get("name").and_then(Value::as_str).unwrap_or("");
        writeln!(w, "Created plan entry '{name}' with status 'pending'.")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AmendBody {
    plan: PlanRef,
    action: PlanAction,
    entry: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum PlanAction {
    Create,
    Amend,
}

impl Render for AmendBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let name = self.entry.get("name").and_then(Value::as_str).unwrap_or("");
        writeln!(w, "Amended plan entry '{name}'.")
    }
}
