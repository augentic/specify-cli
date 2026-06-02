//! Shared CLI output format and the single [`emit`] entry point used by
//! both `specrun` and `specdev`.

use std::io::Write;

use clap::ValueEnum;
use serde::Serialize;
use specify_diagnostics::{DiagnosticReport, Format as DiagnosticsFormat, RenderError, render};
use specify_error::Error;
use specify_standards::lint::ignore::blocking_findings_present;
use specify_workflow::config::Layout;
use specify_workflow::journal::{self, LintScope};

/// Structured (`json`) or human (`text`) CLI output.
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum Format {
    /// Human-readable lines on stdout/stderr.
    Text,
    /// Pretty-printed JSON envelopes for skill/CI consumption.
    Json,
}

/// Emit `payload` through `writer` in the requested format. JSON
/// serialises the body directly via `serde_json::to_writer_pretty`;
/// Text delegates to `render_text`. The single signature covers
/// both success (stdout) and failure (stderr) — there is one entry
/// point for all structured output. Callers construct the locked
/// writer at the boundary so the sink choice is visible at the
/// call site.
///
/// # Errors
///
/// Propagates the underlying serialization or I/O error.
pub fn emit<T: Serialize>(
    writer: &mut dyn Write, format: Format, payload: &T,
    render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
) -> Result<(), Error> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *writer, payload).map_err(|err| Error::Diag {
                code: "json-serialize-failed",
                detail: format!("failed to serialize JSON response: {err}"),
            })?;
            writeln!(writer).map_err(Error::Io)
        }
        Format::Text => render_text(writer, payload).map_err(Error::Io),
    }
}

/// The shared lint-output tail for the `specrun lint` and `specdev lint`
/// handlers (REVIEW.md A19). Carries the already-composed
/// [`DiagnosticReport`] plus everything [`emit_diagnostic_report`] needs
/// to render it, journal the run, and decide blocking status in one
/// place so neither handler re-implements the sequence.
pub struct LintEmit<'a> {
    /// Wire format for the rendered envelope.
    pub format: DiagnosticsFormat,
    /// The composed report to render and journal.
    pub report: &'a DiagnosticReport,
    /// Project (or framework-root) layout the journal event lands under.
    pub layout: Layout<'a>,
    /// Scope facets recorded on the `lint-completed` event.
    pub scope: LintScope,
    /// Command label prefixed onto best-effort journal failures.
    pub command_label: &'static str,
    /// Wall-clock duration of the scan in milliseconds.
    pub elapsed_ms: u128,
    /// Append an extra newline after the (already newline-terminated)
    /// body. `specrun` historically does (`println!`); `specdev` does
    /// not (`print!`) — preserved here so neither surface's stdout shape
    /// shifts under the unification.
    pub trailing_newline: bool,
}

/// Render the lint envelope to stdout, append exactly one
/// `lint-completed` journal event, and report whether any blocking
/// (`critical | important`) finding is present.
///
/// Shared by both lint handlers so the render → print → journal →
/// blocking-decision sequence lives in one place. Callers map the
/// returned [`RenderError`] and the blocking flag onto their own
/// surface conventions (`specrun` → `Result`, `specdev` → `Exit`).
///
/// # Errors
///
/// Propagates the [`RenderError`] from envelope rendering; the journal
/// emit is best-effort and never surfaces as an error.
pub fn emit_diagnostic_report(emit: LintEmit<'_>) -> Result<bool, RenderError> {
    let rendered = render(emit.format, emit.report)?;
    if emit.trailing_newline {
        println!("{rendered}");
    } else {
        print!("{rendered}");
    }
    let blocking = blocking_findings_present(&emit.report.findings);
    let exit_code: i32 = if blocking { 2 } else { 0 };
    journal::emit_lint_completed(
        emit.layout,
        emit.scope,
        &emit.report.findings,
        emit.elapsed_ms,
        exit_code,
        emit.command_label,
    );
    Ok(blocking)
}
