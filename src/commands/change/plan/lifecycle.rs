use std::io::Write;

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::capability::ChangeBrief;
use specify_domain::change::{
    Plan, PlanDoctorDiagnostic, PlanDoctorPayload, Severity, Status, plan_doctor,
};
use specify_domain::config::{InitPolicy, with_state};
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

    let mut results: Vec<ValidateRow> =
        plan_doctor(&plan, Some(&slices_dir), registry.as_ref(), Some(&ctx.project_dir))
            .into_iter()
            .map(ValidateRow::from)
            .collect();

    if let Some(err) = registry_err {
        results.push(ValidateRow {
            level: Severity::Error,
            code: "registry-shape".to_string(),
            message: err.to_string(),
            entry: None,
            data: None,
        });
    }
    if let Some(reg) = &registry {
        let workspace_base = ctx.layout().specify_dir().join("workspace");
        for rp in &reg.projects {
            let slot_project_yaml =
                workspace_base.join(&rp.name).join(".specify").join("project.yaml");
            if slot_project_yaml.exists()
                && let Ok(content) = std::fs::read_to_string(&slot_project_yaml)
                && let Ok(config) = serde_saphyr::from_str::<serde_json::Value>(&content)
                && let Some(slot_capability) = config.get("capability").and_then(|v| v.as_str())
                && slot_capability != rp.capability
            {
                results.push(ValidateRow {
                    level: Severity::Warning,
                    code: "capability-mismatch-workspace".to_string(),
                    message: format!(
                        "workspace clone '{}' has capability '{}' but registry declares '{}'; \
                         the clone's project.yaml is authoritative at execution time",
                        rp.name, slot_capability, rp.capability
                    ),
                    entry: None,
                    data: None,
                });
            }
        }
    }

    let has_errors = results.iter().any(|r| matches!(r.level, Severity::Error));
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
            "run 'specify change plan validate' for detail",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn next(ctx: &Ctx) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ctx.layout().slices_dir();

    let results = plan.validate(Some(&slices_dir), None);
    if results.iter().any(|r| matches!(r.level, Severity::Error)) {
        return Err(Error::validation_failed(
            "plan-structural-errors",
            "plan must be free of structural errors",
            "run 'specify change plan validate' for detail",
        ));
    }

    let body = if let Some(active) = plan.entries.iter().find(|c| c.status == Status::InProgress) {
        NextBody {
            reason: Some("in-progress".into()),
            active: Some(active.name.clone()),
            ..NextBody::default()
        }
    } else if let Some(entry) = plan.next_eligible() {
        NextBody {
            next: Some(entry.name.clone()),
            project: entry.project.clone(),
            capability: entry.capability.clone(),
            description: entry.description.clone(),
            sources: Some(entry.sources.clone()),
            ..NextBody::default()
        }
    } else {
        let all_terminal =
            plan.entries.iter().all(|c| matches!(c.status, Status::Done | Status::Skipped));
        let reason = if all_terminal { "all-done" } else { "stuck" };
        NextBody {
            reason: Some(reason.into()),
            ..NextBody::default()
        }
    };
    ctx.write(&body, write_next_text)?;
    Ok(())
}

pub(super) fn transition(
    ctx: &Ctx, name: String, target: Status, reason: Option<String>,
) -> Result<()> {
    let plan_path = ctx.layout().plan_path();
    let body = with_state::<Plan, _, _>(
        ctx.layout(),
        InitPolicy::RequireExisting("plan.yaml"),
        move |plan| {
            let old_status = plan
                .entries
                .iter()
                .find(|c| c.name == name)
                .ok_or_else(|| Error::Diag {
                    code: "plan-entry-not-found",
                    detail: format!("no slice named '{name}' in plan"),
                })?
                .status;

            plan.transition(&name, target, reason.as_deref())?;

            let entry =
                plan.entries.iter().find(|c| c.name == name).expect("transitioned entry present");
            Ok(TransitionBody {
                plan: plan_ref(plan, &plan_path),
                entry: TransitionRow {
                    name: entry.name.clone(),
                    status: entry.status,
                    status_reason: entry.status_reason.clone(),
                },
                previous_status: old_status,
            })
        },
    )?;
    ctx.write(&body, write_transition_text)?;
    Ok(())
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
    let brief_path = ChangeBrief::path(&ctx.project_dir);
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

/// One row in `specify change plan validate`'s `results[]`.
///
/// Mirrors the historical `Finding` wire shape (`level` / `code` /
/// `message` / `entry`) for backwards compatibility, plus an optional
/// structured `data` payload carried by the four health diagnostics
/// (`cycle-in-depends-on`, `orphan-source-key`, `stale-workspace-clone`,
/// `unreachable-entry`) — codes that previously surfaced through the
/// retired `change plan doctor` verb.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateRow {
    level: Severity,
    code: String,
    message: String,
    entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<PlanDoctorPayload>,
}

impl From<PlanDoctorDiagnostic> for ValidateRow {
    fn from(d: PlanDoctorDiagnostic) -> Self {
        Self {
            level: d.severity,
            code: d.code,
            message: d.message,
            entry: d.entry,
            data: d.data,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanValidateBody<'a> {
    plan: Ref,
    results: &'a [ValidateRow],
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

fn write_validate_row_text(w: &mut dyn Write, row: &ValidateRow) -> std::io::Result<()> {
    let label = if matches!(row.level, Severity::Error) { "ERROR  " } else { "WARNING" };
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
    capability: Option<String>,
    description: Option<String>,
    sources: Option<Vec<String>>,
}

fn write_next_text(w: &mut dyn Write, body: &NextBody) -> std::io::Result<()> {
    if let Some(active) = &body.active {
        writeln!(w, "Active change in progress: {active}")
    } else if let Some(name) = &body.next {
        writeln!(w, "{name}")
    } else if body.reason.as_deref() == Some("all-done") {
        writeln!(w, "All changes done.")
    } else {
        writeln!(
            w,
            "No eligible changes \u{2014} remaining entries are blocked, failed, or waiting on unmet dependencies."
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionBody {
    plan: Ref,
    entry: TransitionRow,
    #[serde(skip)]
    previous_status: Status,
}

fn write_transition_text(w: &mut dyn Write, body: &TransitionBody) -> std::io::Result<()> {
    writeln!(
        w,
        "Transitioned '{}': {} \u{2192} {}.",
        body.entry.name, body.previous_status, body.entry.status
    )
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionRow {
    name: String,
    status: Status,
    status_reason: Option<String>,
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
