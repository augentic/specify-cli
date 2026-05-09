#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to command handlers."
)]

use specify::Error;
use specify_validate::{
    CompatibilityClassification, CompatibilityFinding, CompatibilityReport,
    classify_project_compatibility,
};

use crate::cli::{CompatibilityAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

/// Dispatch `specify compatibility *`.
pub fn run(ctx: &CommandContext, action: CompatibilityAction) -> Result<CliResult, Error> {
    match action {
        CompatibilityAction::Check => check(ctx),
        CompatibilityAction::Report { change } => report(ctx, change),
    }
}

fn check(ctx: &CommandContext) -> Result<CliResult, Error> {
    let report = classify_project_compatibility(&ctx.project_dir, None)?;
    emit_report(ctx.format, &report)?;
    Ok(if report.is_compatible() { CliResult::Success } else { CliResult::ValidationFailed })
}

fn report(ctx: &CommandContext, change: String) -> Result<CliResult, Error> {
    let report = classify_project_compatibility(&ctx.project_dir, Some(change))?;
    emit_report(ctx.format, &report)?;
    Ok(CliResult::Success)
}

fn emit_report(format: OutputFormat, report: &CompatibilityReport) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_response(report)?,
        OutputFormat::Text => print_report(report),
    }
    Ok(())
}

fn print_report(report: &CompatibilityReport) {
    match &report.change {
        Some(change) => println!("compatibility report for change `{change}`"),
        None => println!("compatibility check"),
    }
    println!("checked pairs: {}", report.checked_pairs);
    println!(
        "summary: {} additive, {} breaking, {} ambiguous, {} unverifiable",
        report.summary.additive,
        report.summary.breaking,
        report.summary.ambiguous,
        report.summary.unverifiable
    );
    if report.findings.is_empty() {
        println!("no compatibility findings");
        return;
    }
    for finding in &report.findings {
        print_finding(finding);
    }
}

fn print_finding(finding: &CompatibilityFinding) {
    let kind = finding.change_kind.as_deref().unwrap_or("unclassified");
    println!(
        "- {} [{}] {} -> {} {}",
        classification_label(finding.classification),
        kind,
        finding.producer_project,
        finding.consumer_project,
        finding.producer_contract
    );
    println!("  locator: {}", finding.locator);
    println!("  detail: {}", finding.details);
}

const fn classification_label(classification: CompatibilityClassification) -> &'static str {
    match classification {
        CompatibilityClassification::Additive => "additive",
        CompatibilityClassification::Breaking => "breaking",
        CompatibilityClassification::Ambiguous => "ambiguous",
        CompatibilityClassification::Unverifiable => "unverifiable",
    }
}
