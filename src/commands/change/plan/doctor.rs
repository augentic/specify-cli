//! `specify change plan doctor`.
//!
//! Thin handler over [`specify_change::plan_doctor`]: load the
//! plan + registry, run the doctor pipeline (which is a strict
//! superset of `Plan::validate`), then render the diagnostic stream as
//! text or JSON.

use std::io::Write;

use serde::Serialize;
use specify_change::{Plan, PlanDoctorDiagnostic, PlanDoctorSeverity, plan_doctor};
use specify_config::ProjectConfig;
use specify_error::Result;
use specify_registry::Registry;

use super::{PlanRef, plan_ref, require_file};
use crate::context::Ctx;
use crate::output::{CliResult, Render, Stream, emit};

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
    plan: PlanRef,
    diagnostics: Vec<DiagnosticRow>,
}

impl Render for DoctorBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.diagnostics.is_empty() {
            return writeln!(w, "Plan OK");
        }
        for d in &self.diagnostics {
            let prefix = if d.severity == "error" { "ERROR  " } else { "WARNING" };
            let entry_col = d.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
            writeln!(w, "{prefix} {:<24} {entry_col:<24} {}", d.code, d.message)?;
        }
        Ok(())
    }
}

pub fn run(ctx: &Ctx) -> Result<CliResult> {
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
    let rows: Vec<DiagnosticRow> = diagnostics.iter().map(diagnostic_row).collect();

    emit(
        Stream::Stdout,
        ctx.format,
        &DoctorBody {
            plan: plan_ref(&plan, &plan_path),
            diagnostics: rows,
        },
    )?;

    Ok(if has_errors { CliResult::ValidationFailed } else { CliResult::Success })
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
