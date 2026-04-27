#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use specify::{Error, ValidationReport, ValidationResult, serialize_report, validate_change};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_validate(ctx: &CommandContext, change_dir: PathBuf) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let report = validate_change(&change_dir, &pipeline)?;

    match ctx.format {
        OutputFormat::Json => emit_response(serialize_report(&report)),
        OutputFormat::Text => print_validation_report_text(&report),
    }

    Ok(if report.passed { CliResult::Success } else { CliResult::ValidationFailed })
}

fn print_validation_report_text(report: &ValidationReport) {
    println!("{}", if report.passed { "PASS" } else { "FAIL" });
    for (key, results) in &report.brief_results {
        println!("{key}:");
        for r in results {
            println!("  {}", format_result_line(r));
        }
    }
    if !report.cross_checks.is_empty() {
        println!("cross_checks:");
        for r in &report.cross_checks {
            println!("  {}", format_result_line(r));
        }
    }
}

fn format_result_line(r: &ValidationResult) -> String {
    match r {
        ValidationResult::Pass { rule_id, .. } => format!("[ok] {rule_id}"),
        ValidationResult::Fail { rule_id, detail, .. } => format!("[fail] {rule_id}: {detail}"),
        ValidationResult::Deferred { rule_id, reason, .. } => {
            format!("[defer] {rule_id} ({reason})")
        }
        _ => "[?] unknown validation result".to_string(),
    }
}
