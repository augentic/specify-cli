//! Shared CLI output format and the single [`emit`] entry point used by
//! both `specrun` and `specdev`.

use std::io::Write;

use clap::ValueEnum;
use serde::Serialize;
use specify_error::Error;

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
