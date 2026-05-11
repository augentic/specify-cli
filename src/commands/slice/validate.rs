//! `slice validate` — coherence check against the capability validation rules.

use std::io::Write;

use specify_error::Result;
use specify_validate::{ValidationReport, ValidationResult, serialize_report, validate_slice};

use crate::context::Ctx;
use crate::output::{CliResult, Render, Stream, emit};

pub(super) fn run(ctx: &Ctx, name: String) -> Result<CliResult> {
    let slice_dir = ctx.slices_dir().join(&name);
    let pipeline = ctx.load_pipeline()?;
    let report = validate_slice(&slice_dir, &pipeline)?;
    let exit = if report.passed { CliResult::Success } else { CliResult::ValidationFailed };

    emit(Stream::Stdout, ctx.format, &ValidateBody { report: &report })?;
    Ok(exit)
}

struct ValidateBody<'a> {
    report: &'a ValidationReport,
}

impl serde::Serialize for ValidateBody<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_report(self.report).serialize(serializer)
    }
}

impl Render for ValidateBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{}", if self.report.passed { "PASS" } else { "FAIL" })?;
        for (key, results) in &self.report.brief_results {
            writeln!(w, "{key}:")?;
            for r in results {
                writeln!(w, "  {}", format_result_line(r))?;
            }
        }
        if !self.report.cross_checks.is_empty() {
            writeln!(w, "cross_checks:")?;
            for r in &self.report.cross_checks {
                writeln!(w, "  {}", format_result_line(r))?;
            }
        }
        Ok(())
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
