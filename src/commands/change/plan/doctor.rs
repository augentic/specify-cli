//! `specify change plan doctor` — thin handler over
//! [`specify_domain::change::plan_doctor`] that loads plan + registry,
//! runs the doctor pipeline, and renders the diagnostic stream.

use std::io::Write;

use serde::Serialize;
use specify_domain::change::{Plan, PlanDoctorDiagnostic, PlanDoctorSeverity, plan_doctor};
use specify_domain::registry::Registry;
use specify_error::{Error, Result};

use super::{Ref, plan_ref, require_file};
use crate::context::Ctx;

/// Wire shape of the JSON `diagnostics:` row. Mirrors
/// [`PlanDoctorDiagnostic`] but with `severity` rendered as the
/// kebab-case label string (`error` / `warning`).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DiagnosticRow {
    severity: &'static str,
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorBody {
    plan: Ref,
    diagnostics: Vec<DiagnosticRow>,
}

fn write_doctor_text(w: &mut dyn Write, body: &DoctorBody) -> std::io::Result<()> {
    if body.diagnostics.is_empty() {
        return writeln!(w, "Plan OK");
    }
    for d in &body.diagnostics {
        let prefix = if d.severity == "error" { "ERROR  " } else { "WARNING" };
        let entry_col = d.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
        writeln!(w, "{prefix} {:<24} {entry_col:<24} {}", d.code, d.message)?;
    }
    Ok(())
}

pub(super) fn run(ctx: &Ctx) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ctx.slices_dir();

    // We tolerate a malformed registry by surfacing it as a synthetic
    // diagnostic (matching the `plan validate` posture) so doctor
    // never aborts mid-stream when the registry is the broken thing.
    let (registry, registry_err) = match Registry::load(&ctx.project_dir) {
        Ok(reg) => (reg, None),
        Err(err) => (None, Some(err)),
    };

    let mut diagnostics =
        plan_doctor(&plan, Some(&slices_dir), registry.as_ref(), Some(&ctx.project_dir));
    if let Some(err) = registry_err {
        diagnostics.push(PlanDoctorDiagnostic {
            severity: PlanDoctorSeverity::Error,
            code: "registry-shape".to_string(),
            message: err.to_string(),
            entry: None,
            data: None,
        });
    }

    let has_errors = diagnostics.iter().any(|d| matches!(d.severity, PlanDoctorSeverity::Error));
    let rows: Vec<DiagnosticRow> = diagnostics.iter().map(diagnostic_row).collect();

    ctx.write(
        &DoctorBody {
            plan: plan_ref(&plan, &plan_path),
            diagnostics: rows,
        },
        write_doctor_text,
    )?;

    if has_errors {
        Err(Error::Diag {
            code: "plan-structural-errors",
            detail: "plan has structural errors; run 'specify change plan validate' for detail"
                .to_string(),
        })
    } else {
        Ok(())
    }
}

fn diagnostic_row(d: &PlanDoctorDiagnostic) -> DiagnosticRow {
    let data = d
        .data
        .as_ref()
        .map(|p| serde_json::to_value(p).expect("DiagnosticPayload serialises as JSON"));
    DiagnosticRow {
        severity: d.severity.label(),
        code: d.code.clone(),
        message: d.message.clone(),
        entry: d.entry.clone(),
        data,
    }
}
