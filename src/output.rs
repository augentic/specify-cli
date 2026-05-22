use std::io::Write;
use std::process::ExitCode;

use serde::Serialize;
use specify_error::{Error, ValidationSummary};

use crate::cli::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Exit {
    Success,
    GenericFailure,
    ValidationFailed,
    VersionTooOld,
    /// Argument-shape failure: `clap` exits 2 for unknown flags / missing
    /// arguments; we mirror that for argument errors discovered after
    /// parsing (kebab-case checks, mutually exclusive payloads, etc.).
    ArgumentError,
    /// WASI tool exit-code passthrough; see
    /// [DECISIONS.md §"Exit codes"](../../DECISIONS.md#exit-codes).
    Code(u8),
}

impl Exit {
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::GenericFailure => 1,
            Self::ArgumentError | Self::ValidationFailed => 2,
            Self::VersionTooOld => 3,
            Self::Code(code) => code,
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(r: Exit) -> Self {
        Self::from(r.code())
    }
}

impl From<&Error> for Exit {
    fn from(err: &Error) -> Self {
        match err {
            Error::CliTooOld { .. } => Self::VersionTooOld,
            Error::Validation { .. } => Self::ValidationFailed,
            Error::Argument { .. } => Self::ArgumentError,
            _ => Self::GenericFailure,
        }
    }
}

/// Render `err` as a failure envelope and return the matching exit
/// code. JSON serialises the body directly; Text writes
/// `error: {err}` plus any long-form hint for the variant. Both
/// formats route through [`emit`] against `std::io::stderr()` so
/// failure output never interleaves with the structured success
/// stream skills consume.
///
/// Single dispatcher entry point: handlers return
/// `Result<T, specify_error::Error>` and the run loop in
/// [`crate::commands`] hands the error here. The body shape is
/// always [`ErrorBody`]; for `Error::Validation`, the body's
/// `results` field carries the per-row failures.
pub fn report(format: Format, err: &Error) -> Exit {
    let code = Exit::from(err);
    let body = ErrorBody::from(err);
    let result = emit(Box::new(std::io::stderr().lock()), format, &body, write_error_text);
    if let Err(serialise_err) = result {
        eprintln!("error: {err}");
        eprintln!("error: {serialise_err}");
    }
    code
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
    mut writer: Box<dyn Write>, format: Format, payload: &T,
    render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
) -> Result<(), Error> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut writer, payload).map_err(|err| Error::Diag {
                code: "json-serialize-failed",
                detail: format!("failed to serialize JSON response: {err}"),
            })?;
            writeln!(writer).map_err(Error::Io)
        }
        Format::Text => render_text(&mut writer, payload).map_err(Error::Io),
    }
}

/// Failure envelope used by [`report`] for every error variant. For
/// `Error::Validation`, `results` is `Some(rows)`; otherwise it is
/// `None` and serde elides the field from the JSON output via
/// `skip_serializing_if`.
///
/// Construct via `ErrorBody::from(&err)` — the variant is the only
/// shape on the wire.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ErrorBody<'a> {
    pub(crate) error: String,
    pub(crate) message: String,
    pub(crate) exit_code: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) results: Option<&'a [ValidationSummary]>,
    #[serde(skip)]
    hint: Option<&'static str>,
}

impl<'a> From<&'a Error> for ErrorBody<'a> {
    fn from(err: &'a Error) -> Self {
        let results = match err {
            Error::Validation { results } => Some(results.as_slice()),
            _ => None,
        };
        Self {
            error: err.variant_str(),
            message: err.to_string(),
            exit_code: Exit::from(err).code(),
            results,
            hint: err.hint(),
        }
    }
}

fn write_error_text(w: &mut dyn Write, body: &ErrorBody<'_>) -> std::io::Result<()> {
    writeln!(w, "error: {}", body.message)?;
    if let Some(hint) = body.hint {
        writeln!(w, "hint: {hint}")?;
    }
    Ok(())
}
