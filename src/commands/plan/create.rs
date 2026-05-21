use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify_domain::change::{
    Divergence, Entry, EntryPatch, Patch, Plan, SliceSourceBinding, Status,
};
use specify_domain::config::{InitPolicy, with_state};
use specify_domain::schema::validate_plan;
use specify_error::{Error, Result, is_kebab};

use super::{Ref, check_project, plan_ref};
use crate::cli::{SliceSourceArg, SourceArg};
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
/// `specify plan create` surfaces.
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
/// Callers (`specify plan create`) own the "refuse if any conflicting
/// file exists" pre-flight and the kebab-case validation; this helper
/// is happy to overwrite and assumes `name` is already validated.
pub fn write_scaffold(
    plan_path: &Path, name: &str, sources: BTreeMap<String, String>,
) -> Result<Plan> {
    let plan = Plan::init(name, sources)?;
    plan.save(plan_path)?;
    Ok(plan)
}

/// Materialise CLI `--sources` / `--add-source` arguments into the
/// on-disk [`SliceSourceBinding`] shape, preferring the bare-string
/// shorthand when the candidate id equals the slice's name (RFC-25
/// §`Slice.sources`).
fn binding_from_arg(arg: SliceSourceArg, slice_name: &str) -> SliceSourceBinding {
    match arg.candidate {
        None => SliceSourceBinding::Bare(arg.key),
        Some(candidate) if candidate == slice_name => SliceSourceBinding::Bare(arg.key),
        Some(candidate) => SliceSourceBinding::Structured {
            key: arg.key,
            candidate,
        },
    }
}

/// Map every CLI `--sources` / `--add-source` argument into the
/// on-disk binding shape against `slice_name`.
fn bindings_from_args(args: Vec<SliceSourceArg>, slice_name: &str) -> Vec<SliceSourceBinding> {
    args.into_iter().map(|a| binding_from_arg(a, slice_name)).collect()
}

/// Parse the `--divergence` flag value. Only `accepted` / `rejected`
/// are wire-legal; `none` (absent) is the implicit default and
/// `likely` is reserved for the `/spec:plan` `propose` sub-step.
fn parse_divergence(raw: &str) -> Result<Divergence> {
    match raw {
        "accepted" => Ok(Divergence::Accepted),
        "rejected" => Ok(Divergence::Rejected),
        "none" => Err(Error::Argument {
            flag: "--divergence",
            detail:
                "`none` is the implicit default (absent on disk) and cannot be set explicitly; \
                    omit --divergence to leave the field unchanged"
                    .to_string(),
        }),
        "likely" => Err(Error::Argument {
            flag: "--divergence",
            detail: "`likely` is reserved for the `/spec:plan` propose sub-step and cannot be set \
                    via --divergence; valid operator values are `accepted` and `rejected`"
                .to_string(),
        }),
        other => Err(Error::Argument {
            flag: "--divergence",
            detail: format!(
                "`{other}` is not a valid --divergence value; expected `accepted` or `rejected`"
            ),
        }),
    }
}

/// `specify plan create <name> [--source ...]`. Scaffolds `plan.yaml`
/// only.
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
    ctx: &Ctx, name: String, depends_on: Vec<String>, sources: Vec<SliceSourceArg>,
    description: Option<String>, project: Option<String>, target: Option<String>,
    context: Vec<String>,
) -> Result<()> {
    if let Some(proj) = &project {
        check_project(&ctx.project_dir, proj)?;
    }

    let sources = bindings_from_args(sources, &name);
    let entry = Entry {
        name,
        project,
        target,
        status: Status::Pending,
        depends_on,
        sources,
        context,
        description,
        divergence: None,
    };
    let plan_path = ctx.layout().plan_path();
    let body = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| {
            plan.create(entry)?;
            validate_plan(plan)?;
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
    ctx: &Ctx, name: String, depends_on: Option<Vec<String>>, sources: Option<Vec<SliceSourceArg>>,
    add_source: Vec<SliceSourceArg>, remove_source: Vec<String>, divergence: Option<&str>,
    description: Option<String>, project: Option<String>, target: Option<String>,
    context: Option<Vec<String>>,
) -> Result<()> {
    if let Some(proj) = &project
        && !proj.is_empty()
    {
        check_project(&ctx.project_dir, proj)?;
    }

    let divergence = divergence.map(parse_divergence).transpose()?;
    let plan_path = ctx.layout().plan_path();
    let (body, journal_event) = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| {
            // We materialise per-slice bindings here (rather than in
            // the dispatcher) so the slice-name resolution lines up
            // with the slice we're actually mutating.
            let sources_replace = sources.as_ref().map(|v| bindings_from_args(v.clone(), &name));
            let add_bindings = bindings_from_args(add_source.clone(), &name);

            // Capture pre-amend divergence so the journal event's
            // `from` field carries the implicit-default `none` on the
            // first transition (RFC-25 §Observability).
            let plan_name = plan.name.clone();
            let previous_divergence =
                plan.entries.iter().find(|e| e.name == name).and_then(|e| e.divergence);

            let patch = EntryPatch {
                depends_on: depends_on.clone(),
                sources: sources_replace,
                project: cli_patch(project.clone()),
                target: cli_patch(target.clone()),
                description: cli_patch(description.clone()),
                context: context.clone(),
                divergence,
            };
            plan.amend(&name, patch)?;

            // Apply --add-source / --remove-source after the wholesale
            // `amend` so additive edits compose cleanly with a
            // simultaneous `--sources` replacement.
            if !add_bindings.is_empty() || !remove_source.is_empty() {
                let entry = plan
                    .entries
                    .iter_mut()
                    .find(|c| c.name == name)
                    .expect("amended entry present");
                for key in &remove_source {
                    let before = entry.sources.len();
                    entry.sources.retain(|b| b.key() != key.as_str());
                    if entry.sources.len() == before {
                        return Err(Error::Diag {
                            code: "plan-binding-not-found",
                            detail: format!(
                                "slice `{name}` has no source binding with key `{key}`"
                            ),
                        });
                    }
                }
                for binding in add_bindings {
                    entry.sources.push(binding);
                }
            }

            validate_plan(plan)?;
            let amended =
                plan.entries.iter().find(|c| c.name == name).expect("amended entry present");

            // Build the journal event only when --divergence flipped
            // the slice's `divergence` (RFC-25 §Observability — every
            // operator transition is logged, including no-op writes
            // of the same value).
            let journal_event =
                divergence.map(|to| specify_domain::journal::EventKind::PlanAmendDivergence {
                    plan_name,
                    slice_name: amended.name.clone(),
                    from: specify_domain::journal::DivergenceState::from(previous_divergence),
                    to: specify_domain::journal::DivergenceState::from(Some(to)),
                });

            Ok((
                EntryBody {
                    plan: plan_ref(plan, &plan_path),
                    action: "amend",
                    entry: serde_json::to_value(amended).expect("plan Entry serialises as JSON"),
                },
                journal_event,
            ))
        },
    )?;
    if let Some(kind) = journal_event {
        let event = specify_domain::journal::Event::new(jiff::Timestamp::now(), kind);
        specify_domain::journal::append(ctx.layout(), &event)?;
    }

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
