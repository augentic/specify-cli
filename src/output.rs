use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use specify::{Error, ValidationSummary};

use crate::cli::OutputFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CliResult {
    Success,
    GenericFailure,
    ValidationFailed,
    VersionTooOld,
    /// Argument-shape failure: `clap` exits 2 for unknown flags / missing
    /// arguments; we mirror that for argument errors discovered after
    /// parsing (kebab-case checks, mutually exclusive payloads, etc.).
    ArgumentError,
    Exit(u8),
}

impl CliResult {
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::GenericFailure => 1,
            // Both `Self::ArgumentError` and `Self::ValidationFailed` exit
            // 2 to match clap's parser-error convention. Skills can
            // disambiguate via the kebab-case `error` discriminant in the
            // JSON envelope.
            Self::ArgumentError | Self::ValidationFailed => 2,
            Self::VersionTooOld => 3,
            Self::Exit(code) => code,
        }
    }
}

impl From<CliResult> for ExitCode {
    fn from(r: CliResult) -> Self {
        Self::from(r.code())
    }
}

impl From<&Error> for CliResult {
    fn from(err: &Error) -> Self {
        match err {
            Error::CliTooOld { .. } => Self::VersionTooOld,
            Error::Validation { .. } | Error::ToolDenied(_) | Error::ToolNotDeclared { .. } => {
                Self::ValidationFailed
            }
            Error::Argument { .. } => Self::ArgumentError,
            _ => Self::GenericFailure,
        }
    }
}

/// JSON contract version emitted on every structured response.
/// Bumping it is a breaking change for skill authors.
pub const JSON_SCHEMA_VERSION: u64 = 4;

pub fn emit_error(format: OutputFormat, err: &Error) -> CliResult {
    let code = CliResult::from(err);
    match format {
        OutputFormat::Json => emit_json_error(err, code),
        OutputFormat::Text => {
            eprintln!("error: {err}");
        }
    }
    code
}

#[derive(Serialize)]
pub struct JsonEnvelope<T> {
    #[serde(rename = "schema-version")]
    schema_version: u64,
    #[serde(flatten)]
    payload: T,
}

impl<T: Serialize> JsonEnvelope<T> {
    const fn wrap(payload: T) -> Self {
        Self {
            schema_version: JSON_SCHEMA_VERSION,
            payload,
        }
    }
}

pub fn emit_response<T: Serialize>(payload: T) -> Result<(), Error> {
    use std::io::Write;
    let envelope = JsonEnvelope::wrap(payload);
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &envelope)
        .map_err(|err| Error::Config(format!("failed to serialize JSON response: {err}")))?;
    writeln!(handle).map_err(Error::Io)?;
    Ok(())
}

/// Format-agnostic command-result rendering. JSON is derived from
/// `serde::Serialize`; text rendering is delegated to `render_text`.
///
/// Implementors keep their text/JSON shapes side-by-side in one place,
/// and command dispatchers stop hand-rolling `match ctx.format`.
pub trait Render: Serialize {
    /// Write the human-readable text representation. Implementations
    /// should not append a trailing newline — `emit` adds one when the
    /// renderer leaves stdout mid-line.
    fn render_text(&self, w: &mut dyn std::io::Write) -> std::io::Result<()>;
}

/// Emit `payload` in the requested format. JSON wraps in the standard
/// envelope; Text delegates to `Render::render_text` against locked
/// stdout.
pub fn emit<R: Render>(format: OutputFormat, payload: &R) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_response(payload),
        OutputFormat::Text => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            payload.render_text(&mut handle).map_err(Error::Io)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    pub exit_code: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidationErrorResponse {
    error: String,
    message: String,
    exit_code: u8,
    results: Vec<ValidationResultResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidationResultResponse {
    status: String,
    rule_id: String,
    rule: String,
    detail: Option<String>,
}

pub fn emit_json_error(err: &Error, code: CliResult) {
    if let Error::Validation { results, .. } = err {
        if let Err(serialise_err) = emit_response(ValidationErrorResponse {
            error: "validation".to_string(),
            message: err.to_string(),
            exit_code: code.code(),
            results: results.iter().map(validation_result_response).collect(),
        }) {
            eprintln!("error: {err}");
            eprintln!("error: {serialise_err}");
        }
        return;
    }

    let variant = err.variant_str();
    if let Err(serialise_err) = emit_response(ErrorResponse {
        error: variant.to_string(),
        message: err.to_string(),
        exit_code: code.code(),
    }) {
        eprintln!("error: {err}");
        eprintln!("error: {serialise_err}");
    }
}

fn validation_result_response(summary: &ValidationSummary) -> ValidationResultResponse {
    ValidationResultResponse {
        status: summary.status.to_string(),
        rule_id: summary.rule_id.clone(),
        rule: summary.rule.clone(),
        detail: summary.detail.clone(),
    }
}

pub fn absolute_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .ok()
        .map_or_else(|| path.to_string_lossy().into_owned(), |p| p.to_string_lossy().into_owned())
}
