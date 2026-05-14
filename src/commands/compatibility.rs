pub mod cli;

use std::io::Write;

use specify_domain::validate::{
    CompatibilityFinding, CompatibilityReport, classify_project_compatibility,
};
use specify_error::{Error, Result};

use crate::cli::CompatibilityAction;
use crate::context::Ctx;

/// Dispatch `specify compatibility *`.
pub fn run(ctx: &Ctx, action: CompatibilityAction) -> Result<()> {
    match action {
        CompatibilityAction::Check => check(ctx),
        CompatibilityAction::Report { change } => report(ctx, change),
    }
}

fn check(ctx: &Ctx) -> Result<()> {
    let report = classify_project_compatibility(&ctx.project_dir, None)?;
    let compatible = report.is_compatible();
    ctx.write(&report, write_report_text)?;
    if compatible {
        Ok(())
    } else {
        Err(Error::validation_failed(
            "compatibility-check-failed",
            "cross-project contracts must be compatible",
            "review the compatibility report on stdout for the offending pairs",
        ))
    }
}

fn report(ctx: &Ctx, change: String) -> Result<()> {
    let report = classify_project_compatibility(&ctx.project_dir, Some(change))?;
    ctx.write(&report, write_report_text)?;
    Ok(())
}

fn write_report_text(w: &mut dyn Write, report: &CompatibilityReport) -> std::io::Result<()> {
    match &report.change {
        Some(change) => writeln!(w, "compatibility report for change `{change}`")?,
        None => writeln!(w, "compatibility check")?,
    }
    writeln!(w, "checked pairs: {}", report.checked_pairs)?;
    writeln!(
        w,
        "summary: {} additive, {} breaking, {} ambiguous, {} unverifiable",
        report.summary.additive,
        report.summary.breaking,
        report.summary.ambiguous,
        report.summary.unverifiable
    )?;
    if report.findings.is_empty() {
        return writeln!(w, "no compatibility findings");
    }
    for finding in &report.findings {
        render_finding(w, finding)?;
    }
    Ok(())
}

fn render_finding(w: &mut dyn Write, finding: &CompatibilityFinding) -> std::io::Result<()> {
    let kind = finding.change_kind.as_deref().unwrap_or("unclassified");
    writeln!(
        w,
        "- {} [{}] {} -> {} {}",
        finding.classification,
        kind,
        finding.producer_project,
        finding.consumer_project,
        finding.producer_contract
    )?;
    writeln!(w, "  locator: {}", finding.locator)?;
    writeln!(w, "  detail: {}", finding.details)
}
