use std::io::Write;

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::change::{
    Lifecycle, Plan, PlanDoctorDiagnostic, Severity, SliceSourceBinding, Status, plan_doctor,
};
use specify_domain::config::{InitPolicy, ProjectConfig, with_state};
use specify_domain::registry::Registry;
use specify_error::{Error, Result};

use super::{Ref, plan_ref, require_file};
use crate::context::Ctx;

pub(super) fn validate(ctx: &Ctx) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ctx.layout().slices_dir();

    let (registry, registry_err) = match Registry::load(&ctx.project_dir) {
        Ok(reg) => (reg, None),
        Err(err) => (None, Some(err)),
    };

    let mut results: Vec<PlanDoctorDiagnostic> =
        plan_doctor(&plan, Some(&slices_dir), registry.as_ref(), Some(&ctx.project_dir));

    if let Some(err) = registry_err {
        results.push(PlanDoctorDiagnostic {
            severity: Severity::Error,
            code: "registry-shape".to_string(),
            message: err.to_string(),
            entry: None,
            data: None,
        });
    }
    if let Some(reg) = &registry {
        let workspace_base = ctx.layout().specify_dir().join("workspace");
        for rp in &reg.projects {
            let slot_project_dir = workspace_base.join(&rp.name);
            let slot_project_yaml = slot_project_dir.join(".specify").join("project.yaml");
            if !slot_project_yaml.exists() {
                continue;
            }
            match ProjectConfig::load(&slot_project_dir) {
                Ok(cfg) => {
                    if let Some(slot_adapter) = cfg.adapter.as_deref()
                        && slot_adapter != rp.adapter
                    {
                        results.push(PlanDoctorDiagnostic {
                            severity: Severity::Warning,
                            code: "adapter-mismatch-workspace".to_string(),
                            message: format!(
                                "workspace clone '{}' has adapter '{}' but registry declares '{}'; \
                                 the clone's project.yaml is authoritative at execution time",
                                rp.name, slot_adapter, rp.adapter
                            ),
                            entry: None,
                            data: None,
                        });
                    }
                }
                Err(err) => {
                    results.push(PlanDoctorDiagnostic {
                        severity: Severity::Error,
                        code: "workspace-slot-config-unreadable".to_string(),
                        message: format!(
                            "workspace clone '{}' project.yaml could not be loaded: {err}",
                            rp.name
                        ),
                        entry: None,
                        data: None,
                    });
                }
            }
        }
    }

    let has_errors = results.iter().any(|r| matches!(r.severity, Severity::Error));
    ctx.write(
        &PlanValidateBody {
            plan: Ref {
                name: plan.name,
                path: plan_path.display().to_string(),
            },
            results: &results,
            passed: !has_errors,
        },
        write_plan_validate_text,
    )?;
    if has_errors {
        Err(Error::validation_failed(
            "plan-structural-errors",
            "plan must be free of structural errors",
            "run 'specify plan validate' for detail",
        ))
    } else {
        Ok(())
    }
}

/// `specify plan next` — return the active in-progress entry, or
/// transition the next eligible `Pending` entry to `InProgress` and
/// return it. The only writer of per-entry `in-progress` per
/// RFC-25 §CLI surface.
pub(super) fn next(ctx: &Ctx) -> Result<()> {
    let slices_dir = ctx.layout().slices_dir();

    let body = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| {
            let validate_results = plan.validate(Some(&slices_dir), None);
            if validate_results.iter().any(|r| matches!(r.level, Severity::Error)) {
                return Err(Error::validation_failed(
                    "plan-structural-errors",
                    "plan must be free of structural errors",
                    "run 'specify plan validate' for detail",
                ));
            }

            // RFC-25 §CLI surface: "plan next returns the active
            // in-progress entry before selecting a new pending entry,
            // and reports drained only when no active or pending
            // entries remain."
            let was_executing = plan.is_executing();
            let advanced = plan.advance_next()?;
            Ok(match advanced {
                None => {
                    let reason = if plan.is_drained() { "drained" } else { "stuck" };
                    NextBody {
                        reason: Some(reason.into()),
                        ..NextBody::default()
                    }
                }
                Some(entry) if was_executing => NextBody {
                    reason: Some("in-progress".into()),
                    active: Some(entry.name.clone()),
                    ..NextBody::default()
                },
                Some(entry) => NextBody {
                    next: Some(entry.name.clone()),
                    project: entry.project.clone(),
                    target: entry.target.clone(),
                    description: entry.description.clone(),
                    sources: Some(entry.sources.clone()),
                    ..NextBody::default()
                },
            })
        },
    )?;
    ctx.write(&body, write_next_text)?;
    Ok(())
}

/// `specify plan transition <name> <target>` — dispatches to either
/// the plan-level Gate 1 stamp (`<plan-name> reviewed`) or the
/// per-entry close (`<entry-name> done`). Any other combination is
/// rejected with `Error::Argument` (exit 2).
///
/// `<plan-name> reviewed` against an already-reviewed plan is an
/// idempotent no-op (exit 0, no journal event) per RFC-27 §D7 —
/// running the explicit transition after `specify plan create
/// --auto-review` must not double-stamp the lifecycle nor double-
/// fire `plan.transition.reviewed`.
pub(super) fn transition(ctx: &Ctx, name: String, target: String) -> Result<()> {
    let plan_path = ctx.layout().plan_path();
    let body = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| dispatch_transition(plan, &plan_path, &name, &target),
    )?;
    // RFC-25 §Observability: Gate-1 stamp emits a journal event when
    // the lifecycle actually changed. The same-state no-op path
    // (already `reviewed`) flags `changed = false` so we skip the
    // emit — preserves single-emit-per-event for the
    // `plan.transition.reviewed` line.
    if matches!(body.kind, TransitionKind::Plan) && body.changed {
        let event = specify_domain::journal::Event::new(
            Timestamp::now(),
            specify_domain::journal::EventKind::PlanTransitionReviewed {
                plan_name: body.name.clone(),
            },
        );
        specify_domain::journal::append_batch(ctx.layout(), std::slice::from_ref(&event))?;
    }
    ctx.write(&body, write_transition_text)?;
    Ok(())
}

fn dispatch_transition(
    plan: &mut Plan, plan_path: &std::path::Path, name: &str, target: &str,
) -> Result<TransitionBody> {
    if name == plan.name {
        // Plan-level transition: only `reviewed` is legal.
        return match target {
            "reviewed" => {
                let previous = plan.lifecycle;
                if matches!(previous, Lifecycle::Reviewed) {
                    // RFC-27 §D7: `--auto-review` already stamped
                    // this plan; the explicit transition is the
                    // operator's belt-and-braces follow-up. No
                    // disk or journal write — `body.changed` is
                    // `false` so the caller suppresses the emit.
                    return Ok(TransitionBody {
                        plan: plan_ref(plan, plan_path),
                        kind: TransitionKind::Plan,
                        name: plan.name.clone(),
                        previous: previous.to_string(),
                        current: plan.lifecycle.to_string(),
                        changed: false,
                    });
                }
                plan.transition_lifecycle(Lifecycle::Reviewed)?;
                Ok(TransitionBody {
                    plan: plan_ref(plan, plan_path),
                    kind: TransitionKind::Plan,
                    name: plan.name.clone(),
                    previous: previous.to_string(),
                    current: plan.lifecycle.to_string(),
                    changed: true,
                })
            }
            other => Err(plan_target_invalid(other)),
        };
    }

    // Per-entry transition: only `done` is legal. `pending` is owned by
    // `plan add`/`amend`; `in-progress` is owned by `plan next`; and
    // `blocked`/`failed`/`skipped` are not v1 states.
    match target {
        "done" => {
            let idx =
                plan.entries.iter().position(|e| e.name == name).ok_or_else(|| Error::Diag {
                    code: "plan-entry-not-found",
                    detail: format!("no slice named '{name}' in plan"),
                })?;
            let previous = plan.entries[idx].status;
            plan.transition(name, Status::Done)?;
            let entry = &plan.entries[idx];
            Ok(TransitionBody {
                plan: plan_ref(plan, plan_path),
                kind: TransitionKind::Entry,
                name: entry.name.clone(),
                previous: previous.to_string(),
                current: entry.status.to_string(),
                changed: true,
            })
        }
        other => Err(entry_target_invalid(other)),
    }
}

fn plan_target_invalid(target: &str) -> Error {
    Error::Argument {
        flag: "<target>",
        detail: format!(
            "plan-level transition target must be `reviewed`; got `{target}`. \
             Run `specify plan transition <plan-name> reviewed` to stamp Gate 1."
        ),
    }
}

fn entry_target_invalid(target: &str) -> Error {
    Error::Argument {
        flag: "<target>",
        detail: match target {
            "pending" => {
                "per-entry `pending` is written by `plan add` / `plan amend`, not `plan transition`. \
                 To clear an entry, drop and re-add it.".to_string()
            }
            "in-progress" => {
                "per-entry `in-progress` is written only by `plan next`; \
                 `plan transition` cannot move an entry into the active slot."
                    .to_string()
            }
            "blocked" | "failed" | "skipped" => format!(
                "per-entry `{target}` is not a v1 state — RFC-25 collapsed the per-entry enum to \
                 `pending | in-progress | done`. Build failures and merge conflicts leave the \
                 active entry `in-progress`."
            ),
            other => format!(
                "per-entry transition target must be `done`; got `{other}`. \
                 `done` is stamped by `/spec:merge` (or by hand once the slice is merged)."
            ),
        },
    }
}

pub(super) fn archive(ctx: &Ctx, force: bool) -> Result<()> {
    let layout = ctx.layout();
    let plan_path = layout.plan_path();
    if !plan_path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path: plan_path,
        });
    }
    let archive_dir = layout.archive_dir().join("plans");
    let brief_path = layout.change_brief_path();
    let plan_name = Plan::load(&plan_path)?.name;

    let (archived, archived_plans_dir) =
        Plan::archive(&plan_path, &brief_path, &archive_dir, force, Timestamp::now())?;
    ctx.write(
        &ArchiveBody {
            archived: archived.display().to_string(),
            archived_plans_dir: archived_plans_dir.as_deref().map(|p| p.display().to_string()),
            plan: ArchivedPlan { name: plan_name },
        },
        write_archive_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanValidateBody<'a> {
    plan: Ref,
    results: &'a [PlanDoctorDiagnostic],
    passed: bool,
}

fn write_plan_validate_text(w: &mut dyn Write, body: &PlanValidateBody<'_>) -> std::io::Result<()> {
    if body.results.is_empty() {
        return writeln!(w, "Plan OK");
    }
    for row in body.results {
        write_validate_row_text(w, row)?;
    }
    Ok(())
}

fn write_validate_row_text(w: &mut dyn Write, row: &PlanDoctorDiagnostic) -> std::io::Result<()> {
    let label = if matches!(row.severity, Severity::Error) { "ERROR  " } else { "WARNING" };
    let entry_col = row.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
    writeln!(w, "{label} {:<32} {:<24} {}", row.code, entry_col, row.message)
}

#[derive(Serialize, Default)]
#[serde(rename_all = "kebab-case")]
struct NextBody {
    next: Option<String>,
    reason: Option<String>,
    active: Option<String>,
    project: Option<String>,
    target: Option<String>,
    description: Option<String>,
    sources: Option<Vec<SliceSourceBinding>>,
}

fn write_next_text(w: &mut dyn Write, body: &NextBody) -> std::io::Result<()> {
    if let Some(active) = &body.active {
        writeln!(w, "Active change in progress: {active}")
    } else if let Some(name) = &body.next {
        writeln!(w, "{name}")
    } else if body.reason.as_deref() == Some("drained") {
        writeln!(w, "Plan drained — no per-entry pending or in-progress remains.")
    } else {
        writeln!(
            w,
            "No eligible changes \u{2014} remaining entries are waiting on unmet dependencies."
        )
    }
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum TransitionKind {
    Plan,
    Entry,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionBody {
    plan: Ref,
    kind: TransitionKind,
    name: String,
    previous: String,
    current: String,
    /// `false` when the transition was an idempotent no-op (RFC-27
    /// §D7 — explicit `reviewed` after `--auto-review`); `true`
    /// when the lifecycle / status actually moved. The outer
    /// handler reads this to decide whether to fire the
    /// `plan.transition.reviewed` journal event.
    #[serde(skip)]
    changed: bool,
}

fn write_transition_text(w: &mut dyn Write, body: &TransitionBody) -> std::io::Result<()> {
    match body.kind {
        TransitionKind::Plan if !body.changed => {
            writeln!(w, "Plan '{}' is already at lifecycle: {} (no-op).", body.name, body.current)
        }
        TransitionKind::Plan => writeln!(
            w,
            "Stamped plan '{}': lifecycle {} \u{2192} {}.",
            body.name, body.previous, body.current
        ),
        TransitionKind::Entry => writeln!(
            w,
            "Transitioned '{}': {} \u{2192} {}.",
            body.name, body.previous, body.current
        ),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArchiveBody {
    archived: String,
    archived_plans_dir: Option<String>,
    plan: ArchivedPlan,
}

fn write_archive_text(w: &mut dyn Write, body: &ArchiveBody) -> std::io::Result<()> {
    match &body.archived_plans_dir {
        Some(dir) => {
            writeln!(w, "Archived plan to {}. Working directory moved to {dir}.", body.archived)
        }
        None => writeln!(w, "Archived plan to {}.", body.archived),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArchivedPlan {
    name: String,
}
