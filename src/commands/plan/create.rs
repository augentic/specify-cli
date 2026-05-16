use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify_domain::change::{Entry, EntryPatch, Patch, Plan, Status};
use specify_domain::config::{InitPolicy, with_state};
use specify_error::{Error, Result, is_kebab};

use super::{Ref, check_project, plan_ref};
use crate::cli::SourceArg;
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

/// Validate `--source key=value` arguments and collapse them into the
/// `BTreeMap` shape `Plan::init` expects. Refuses duplicate keys with
/// the stable `plan-source-duplicate-key` diagnostic.
pub fn build_source_map(sources: Vec<SourceArg>) -> Result<BTreeMap<String, String>> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for SourceArg { key, value } in sources {
        if map.contains_key(&key) {
            return Err(Error::Diag {
                code: "plan-source-duplicate-key",
                detail: format!("duplicate key `{key}` in --source arguments"),
            });
        }
        map.insert(key, value);
    }
    Ok(map)
}

/// Validate `name` is kebab-case. Mirrors the diagnostic code that
/// `specify change draft` and `specify plan create` both surface.
pub fn require_kebab_change_name(name: &str) -> Result<()> {
    if !is_kebab(name) {
        return Err(Error::Diag {
            code: "change-name-not-kebab",
            detail: format!(
                "change: name `{name}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
            ),
        });
    }
    Ok(())
}

/// Build the in-memory [`Plan`] and write it atomically to `plan_path`.
///
/// Callers (`specify plan create` and `specify change draft`) own the
/// "refuse if any conflicting file exists" pre-flight and the
/// kebab-case validation; this helper is happy to overwrite and assumes
/// `name` is already validated.
pub fn write_scaffold(
    plan_path: &Path, name: &str, sources: BTreeMap<String, String>,
) -> Result<Plan> {
    let plan = Plan::init(name, sources)?;
    plan.save(plan_path)?;
    Ok(plan)
}

/// `specify plan create <name> [--source ...]`. Scaffolds `plan.yaml`
/// only — the joint scaffolder that also writes `change.md` is
/// `specify change draft`, which delegates here for the plan half.
pub(super) fn create(ctx: &Ctx, name: String, sources: Vec<SourceArg>) -> Result<()> {
    require_kebab_change_name(&name)?;
    let source_map = build_source_map(sources)?;
    let plan_path = ctx.layout().plan_path();
    if plan_path.exists() {
        return Err(Error::Diag {
            code: "already-exists",
            detail: format!("refusing to overwrite existing plan at {}", plan_path.display()),
        });
    }
    write_scaffold(&plan_path, &name, source_map)?;
    ctx.write(
        &CreateBody {
            name,
            plan: plan_path.display().to_string(),
        },
        write_create_text,
    )?;
    Ok(())
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
                action: "create",
                entry: serde_json::to_value(created).expect("plan Entry serialises as JSON"),
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
                action: "amend",
                entry: serde_json::to_value(amended).expect("plan Entry serialises as JSON"),
            })
        },
    )?;

    ctx.write(&body, write_entry_text)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    name: String,
    plan: String,
}

fn write_create_text(w: &mut dyn Write, body: &CreateBody) -> std::io::Result<()> {
    writeln!(w, "Initialised plan '{}' at {}.", body.name, body.plan)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryBody {
    plan: Ref,
    action: &'static str,
    entry: Value,
}

fn write_entry_text(w: &mut dyn Write, body: &EntryBody) -> std::io::Result<()> {
    let name = body.entry.get("name").and_then(Value::as_str).unwrap_or("");
    match body.action {
        "create" => writeln!(w, "Created plan entry '{name}' with status 'pending'."),
        "amend" => writeln!(w, "Amended plan entry '{name}'."),
        other => unreachable!("unexpected EntryBody action: {other}"),
    }
}
