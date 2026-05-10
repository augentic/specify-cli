//! `specify change plan doctor`.
//!
//! Thin handler over [`specify_change::plan_doctor`]: load the
//! plan + registry, run the doctor pipeline (which is a strict
//! superset of `Plan::validate`), then render the diagnostic stream as
//! text or JSON.

use std::io::Write;

use serde::Serialize;
use serde_json::Value;
use specify_change::{Plan, PlanDoctorDiagnostic, PlanDoctorSeverity, plan_doctor};
use specify_config::ProjectConfig;
use specify_error::Error;
use specify_registry::Registry;

use super::{PlanRef, plan_ref, require_file};
use crate::context::CommandContext;
use crate::output::{CliResult, Render, emit};

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
}

impl Render for DoctorBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.diagnostics.is_empty() {
            return writeln!(w, "Plan OK");
        }
        for d in &self.diagnostics {
            let severity = d.get("severity").and_then(Value::as_str).unwrap_or("");
            let prefix = if severity == "error" { "ERROR  " } else { "WARNING" };
            let code = d.get("code").and_then(Value::as_str).unwrap_or("");
            let message = d.get("message").and_then(Value::as_str).unwrap_or("");
            let entry_col = d
                .get("entry")
                .and_then(Value::as_str)
                .map_or_else(String::new, |e| format!("[{e}]"));
            writeln!(w, "{prefix} {code:<24} {entry_col:<24} {message}")?;
        }
        Ok(())
    }
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
    let rows: Vec<Value> = diagnostics.iter().map(diagnostic_to_json).collect();

    emit(
        ctx.format,
        &DoctorBody {
            plan: plan_ref(&plan, &plan_path),
            diagnostics: rows,
        },
    )?;

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
