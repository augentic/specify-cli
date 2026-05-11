pub mod cli;

use std::io::Write;

use specify_error::Result;
use specify_validate::{
    CompatibilityClassification, CompatibilityFinding, CompatibilityReport,
    classify_project_compatibility,
};

use crate::cli::CompatibilityAction;
use crate::context::Ctx;
use crate::output::{CliResult, Render, Stream, emit};

/// Dispatch `specify compatibility *`.
pub fn run(ctx: &Ctx, action: CompatibilityAction) -> Result<CliResult> {
    match action {
        CompatibilityAction::Check => check(ctx),
        CompatibilityAction::Report { change } => report(ctx, change).map(|()| CliResult::Success),
    }
}

fn check(ctx: &Ctx) -> Result<CliResult> {
    let report = classify_project_compatibility(&ctx.project_dir, None)?;
    emit(Stream::Stdout, ctx.format, &report)?;
    Ok(if report.is_compatible() { CliResult::Success } else { CliResult::ValidationFailed })
}

fn report(ctx: &Ctx, change: String) -> Result<()> {
    let report = classify_project_compatibility(&ctx.project_dir, Some(change))?;
    emit(Stream::Stdout, ctx.format, &report)?;
    Ok(())
}

impl Render for CompatibilityReport {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match &self.change {
            Some(change) => writeln!(w, "compatibility report for change `{change}`")?,
            None => writeln!(w, "compatibility check")?,
        }
        writeln!(w, "checked pairs: {}", self.checked_pairs)?;
        writeln!(
            w,
            "summary: {} additive, {} breaking, {} ambiguous, {} unverifiable",
            self.summary.additive,
            self.summary.breaking,
            self.summary.ambiguous,
            self.summary.unverifiable
        )?;
        if self.findings.is_empty() {
            return writeln!(w, "no compatibility findings");
        }
        for finding in &self.findings {
            render_finding(w, finding)?;
        }
        Ok(())
    }
}

fn render_finding(w: &mut dyn Write, finding: &CompatibilityFinding) -> std::io::Result<()> {
    let kind = finding.change_kind.as_deref().unwrap_or("unclassified");
    writeln!(
        w,
        "- {} [{}] {} -> {} {}",
        classification_label(finding.classification),
        kind,
        finding.producer_project,
        finding.consumer_project,
        finding.producer_contract
    )?;
    writeln!(w, "  locator: {}", finding.locator)?;
    writeln!(w, "  detail: {}", finding.details)
}

const fn classification_label(classification: CompatibilityClassification) -> &'static str {
    match classification {
        CompatibilityClassification::Additive => "additive",
        CompatibilityClassification::Breaking => "breaking",
        CompatibilityClassification::Ambiguous => "ambiguous",
        CompatibilityClassification::Unverifiable => "unverifiable",
    }
}
