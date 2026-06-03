//! `slice validate` — coherence check against the adapter validation
//! rules plus first-use schema validation of per-source `Evidence`
//! files and workflow §Requirement block contract validation of
//! `spec.md` provenance metadata.
//!
//! The pre-adapter gate kernel lives in
//! [`specify_workflow::slice::validate`]; this handler orchestrates it
//! against the adapter rules (`specify_validate::validate_slice`, which
//! the workflow layer may not depend on), renders the report on stdout,
//! and maps the blocking decision to exit 2.

use specify_diagnostics::{
    Diagnostic, DiagnosticReport, DiagnosticReportVersion, DiagnosticSummary, blocking_present,
    renumber,
};
use specify_error::{Error, Result};
use specify_validate::validate_slice;
use specify_workflow::slice::validate::{PreAdapter, append_synthesis_journal, pre_adapter_gates};

use crate::runtime::context::Ctx;

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    match pre_adapter_gates(ctx.layout(), name)? {
        PreAdapter::Gate { code, findings } => fail_with(ctx, code, findings),
        PreAdapter::Proceed {
            synthesis_tags,
            mut advisories,
        } => {
            // Adapter validation findings — `validate_slice` returns one
            // `violation` diagnostic per structural Fail and one `review`
            // diagnostic per deferred semantic rule. The non-blocking
            // `discovery-lead-synopsis-thin` advisories ride this surface
            // too; only a blocking diagnostic gates exit.
            let mut findings = validate_slice(&ctx.slices_dir().join(name))?;
            findings.append(&mut advisories);
            let blocking = blocking_present(&findings);
            render_report(ctx, findings)?;

            if blocking {
                Err(Error::validation_failed(
                    "slice-validation-failed",
                    "slice must satisfy adapter validation",
                    format!("slice `{name}` failed validation"),
                ))
            } else {
                // `slice.synthesis.{conflict,divergence,unknown}` emit
                // once per tagged requirement after a successful validate.
                append_synthesis_journal(ctx.layout(), name, synthesis_tags)?;
                Ok(())
            }
        }
    }
}

/// Render `findings` as a [`DiagnosticReport`] on stdout in the active
/// `Ctx` format. JSON serialises the wire envelope; text renders a
/// PASS/FAIL banner plus one line per diagnostic. Ids are assigned
/// sequentially at render time.
fn render_report(ctx: &Ctx, mut findings: Vec<Diagnostic>) -> Result<()> {
    renumber(&mut findings);
    let blocking = blocking_present(&findings);
    let report = DiagnosticReport {
        version: DiagnosticReportVersion,
        summary: DiagnosticSummary::from_diagnostics(&findings),
        findings,
    };
    ctx.write(&report, move |w, report| {
        writeln!(w, "{}", if blocking { "FAIL" } else { "PASS" })?;
        for finding in &report.findings {
            writeln!(w, "  {}", format_finding_line(finding))?;
        }
        Ok(())
    })
}

/// Render `findings` on stdout and return the payload-free
/// [`Error::Validation`] keyed on `code`. Used by every pre-adapter
/// gate so the operator sees the full diagnostic surface before the
/// gate fails the command.
fn fail_with(ctx: &Ctx, code: &'static str, findings: Vec<Diagnostic>) -> Result<()> {
    let count = findings.len();
    render_report(ctx, findings)?;
    Err(Error::validation_failed(
        code,
        "slice must satisfy structural invariants",
        format!("{count} blocking finding(s)"),
    ))
}

/// One-line text rendering of a diagnostic for the PASS/FAIL banner.
/// `violation` findings are blocking defects (`[fail]`); `review`
/// findings are deferred requests for judgment (`[review]`).
fn format_finding_line(d: &Diagnostic) -> String {
    let rule = d.rule_id.as_deref().unwrap_or("<unknown>");
    match d.kind {
        specify_diagnostics::DiagnosticKind::Violation => {
            format!("[fail] {}: {}", rule, d.impact)
        }
        specify_diagnostics::DiagnosticKind::Review => {
            format!("[review] {} ({})", rule, d.impact)
        }
    }
}
