use std::io::Write;

use serde::Serialize;
use serde_json::Value;
use specify_change::{Entry, EntryPatch, Status};
use specify_error::Result;

use super::{PlanRef, change_entry_json, check_project, load_for_write, plan_ref};
use crate::context::Ctx;
use crate::output::{Render, Stream, emit};

pub(super) fn add(
    ctx: &Ctx, name: String, depends_on: Vec<String>, sources: Vec<String>,
    description: Option<String>, project: Option<String>, capability: Option<String>,
    context: Vec<String>,
) -> Result<()> {
    let (plan_path, mut plan) = load_for_write(ctx)?;

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

    plan.create(entry)?;
    plan.save(&plan_path)?;

    let created = plan.entries.last().expect("Plan::create appended an entry that is now missing");

    emit(
        Stream::Stdout,
        ctx.format,
        &AddBody {
            plan: plan_ref(&plan, &plan_path),
            action: "create",
            entry: change_entry_json(created),
        },
    )?;
    Ok(())
}

pub(super) fn amend(
    ctx: &Ctx, name: String, depends_on: Option<Vec<String>>,
    sources: Option<Vec<String>>, description: Option<String>, project: Option<String>,
    capability: Option<String>, context: Option<Vec<String>>,
) -> Result<()> {
    let (plan_path, mut plan) = load_for_write(ctx)?;

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

    plan.amend(&name, patch)?;
    plan.save(&plan_path)?;

    let amended = plan.entries.iter().find(|c| c.name == name).expect("amended entry present");

    emit(
        Stream::Stdout,
        ctx.format,
        &AmendBody {
            plan: plan_ref(&plan, &plan_path),
            action: "amend",
            entry: change_entry_json(amended),
        },
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AddBody {
    plan: PlanRef,
    action: &'static str,
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
    action: &'static str,
    entry: Value,
}

impl Render for AmendBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let name = self.entry.get("name").and_then(Value::as_str).unwrap_or("");
        writeln!(w, "Amended plan entry '{name}'.")
    }
}
