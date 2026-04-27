use std::path::PathBuf;

use specify::{ValidationReport, ValidationResult, serialize_report, validate_change};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_json};

pub(crate) fn run_validate(format: OutputFormat, change_dir: PathBuf) -> CliResult {
    let ctx = match CommandContext::require(format) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let pipeline = match ctx.load_pipeline() {
        Ok(view) => view,
        Err(code) => return code,
    };
    let report = match validate_change(&change_dir, &pipeline) {
        Ok(report) => report,
        Err(err) => return ctx.emit_error(&err),
    };

    match format {
        OutputFormat::Json => emit_json(serialize_report(&report)),
        OutputFormat::Text => print_validation_report_text(&report),
    }

    if report.passed { CliResult::Success } else { CliResult::ValidationFailed }
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
            _ => unreachable!(),
    }
}

