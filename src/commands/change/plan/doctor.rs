//! `specify change plan doctor` — RFC-9 §4B.
//!
//! Thin handler over [`specify_change::plan_doctor`]: load the
//! plan + registry, run the doctor pipeline (which is a strict
//! superset of `Plan::validate`), then render the diagnostic stream as
//! text or JSON.

use serde::Serialize;
use serde_json::Value;
use specify_change::{Plan, PlanDoctorDiagnostic, PlanDoctorSeverity, plan_doctor};
use specify_config::ProjectConfig;
use specify_error::Error;
use specify_registry::Registry;

use super::{PlanRef, plan_ref, require_file};
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

/// Wire shape of the JSON `diagnostics:` row. Mirrors
/// [`PlanDoctorDiagnostic`] but with `severity` rendered as the
/// kebab-case label string (`error` / `warning`).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DiagnosticRow<'a> {
    severity: &'a str,
    code: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DoctorBody {
    plan: PlanRef,
    diagnostics: Vec<Value>,
    ok: bool,
}

pub fn run(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ProjectConfig::slices_dir(&ctx.project_dir);

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

    match ctx.format {
        OutputFormat::Json => {
            let rows: Vec<Value> = diagnostics.iter().map(diagnostic_to_json).collect();
            emit_response(DoctorBody {
                plan: plan_ref(&plan, &plan_path),
                diagnostics: rows,
                ok: !has_errors,
            })?;
        }
        OutputFormat::Text => render_text(&diagnostics),
    }

    Ok(if has_errors { CliResult::ValidationFailed } else { CliResult::Success })
}

fn diagnostic_to_json(d: &PlanDoctorDiagnostic) -> Value {
    // Two-step serialise so we can flatten the structured `data`
    // payload into the same JSON object every diagnostic exposes,
    // independent of which `code` produced it. `serde_json::to_value`
    // never fails for well-typed structs.
    let data = d
        .data
        .as_ref()
        .map(|p| serde_json::to_value(p).expect("DiagnosticPayload serialises as JSON"));
    serde_json::to_value(DiagnosticRow {
        severity: d.severity.label(),
        code: &d.code,
        message: &d.message,
        entry: d.entry.as_deref(),
        data,
    })
    .expect("DiagnosticRow serialises as JSON")
}

fn render_text(diagnostics: &[PlanDoctorDiagnostic]) {
    if diagnostics.is_empty() {
        println!("Plan OK");
        return;
    }
    for d in diagnostics {
        let prefix = match d.severity {
            PlanDoctorSeverity::Error => "ERROR  ",
            PlanDoctorSeverity::Warning => "WARNING",
        };
        let entry_col = d.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
        println!("{prefix} {:<24} {:<24} {}", d.code, entry_col, d.message);
    }
}
