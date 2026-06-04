use std::io::Write;
use std::process::ExitCode;

use serde::Serialize;
use specify_error::Error;

pub use crate::output::{Format, emit};

/// Process exit code the CLI returns, mapped from a handler result.
///
/// [`Exit::from`] (`&Error`) is the single source of truth for the
/// failure mapping; see DECISIONS.md §"Exit codes".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Exit {
    /// Command succeeded (exit 0).
    Success,
    /// Any error without a more specific code (exit 1).
    GenericFailure,
    /// Validation findings or `Error::Validation` (exit 2).
    ValidationFailed,
    /// `Error::CliTooOld` — the binary is older than the project floor (exit 3).
    VersionTooOld,
    /// The project's pinned `specify_version` major is older than the
    /// running binary; the operator must run `specify migrate` first.
    MigrationRequired,
    /// Argument-shape failure: `clap` exits 2 for unknown flags / missing
    /// arguments; we mirror that for argument errors discovered after
    /// parsing (kebab-case checks, mutually exclusive payloads, etc.).
    ArgumentError,
    /// WASI tool exit-code passthrough; see
    /// [DECISIONS.md §"Exit codes"](../../DECISIONS.md#exit-codes).
    Code(u8),
}

impl Exit {
    /// Numeric process exit code for this outcome.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::GenericFailure => 1,
            Self::ArgumentError | Self::ValidationFailed => 2,
            Self::VersionTooOld => 3,
            Self::MigrationRequired => 4,
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
            Error::ProjectNeedsMigration { .. } => Self::MigrationRequired,
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
/// [`crate::runtime::commands`] hands the error here. The body shape is
/// always [`ErrorBody`]. `Error::Validation` is payload-free — its
/// `code` becomes the wire `error` discriminant and its `detail` the
/// `message`; per-finding rows are rendered by the producing handler on
/// stdout as a [`specify_diagnostics::DiagnosticReport`] before the
/// payload-free error is returned.
pub fn report(format: Format, err: &Error) -> Exit {
    let code = Exit::from(err);
    let body = ErrorBody::from(err);
    let result = emit(&mut std::io::stderr().lock(), format, &body, write_error_text);
    if let Err(serialise_err) = result {
        eprintln!("error: {err}");
        eprintln!("error: {serialise_err}");
    }
    code
}

/// Failure envelope used by [`report`] for every error variant. The
/// shape is now payload-free: `error` carries the variant discriminant
/// (the `code` for `Error::Validation`), `message` the rendered detail,
/// and `exit-code` the numeric exit. Per-finding rows are no longer
/// part of the error body — handlers render
/// [`specify_diagnostics::DiagnosticReport`] on stdout before returning
/// the payload-free error.
///
/// Construct via `ErrorBody::from(&err)` — the variant is the only
/// shape on the wire.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ErrorBody {
    pub(crate) error: std::borrow::Cow<'static, str>,
    pub(crate) message: String,
    pub(crate) exit_code: u8,
    #[serde(skip)]
    hint: Option<&'static str>,
}

impl From<&Error> for ErrorBody {
    fn from(err: &Error) -> Self {
        Self {
            error: err.variant_str(),
            message: err.to_string(),
            exit_code: Exit::from(err).code(),
            hint: err.hint(),
        }
    }
}

fn write_error_text(w: &mut dyn Write, body: &ErrorBody) -> std::io::Result<()> {
    writeln!(w, "error: {}", body.message)?;
    if let Some(hint) = body.hint {
        writeln!(w, "hint: {hint}")?;
    }
    Ok(())
}
