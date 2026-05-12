//! `slice validate` — coherence check against the capability validation rules.

use specify_domain::validate::{ValidationResult, serialize_report, validate_slice};
use specify_error::{Error, Result};

use crate::context::Ctx;

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let pipeline = ctx.load_pipeline()?;
    let report = validate_slice(&slice_dir, &pipeline)?;
    let passed = report.passed;

    ctx.emit_with(&serialize_report(&report), |w, _| {
        writeln!(w, "{}", if report.passed { "PASS" } else { "FAIL" })?;
        for (key, results) in &report.brief_results {
            writeln!(w, "{key}:")?;
            for r in results {
                writeln!(w, "  {}", format_result_line(r))?;
            }
        }
        if !report.cross_checks.is_empty() {
            writeln!(w, "cross_checks:")?;
            for r in &report.cross_checks {
                writeln!(w, "  {}", format_result_line(r))?;
            }
        }
        Ok(())
    })?;
    if passed {
        Ok(())
    } else {
        Err(Error::Diag {
            code: "slice-validation-failed",
            detail: format!("slice `{name}` failed validation"),
        })
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
