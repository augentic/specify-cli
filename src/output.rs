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
    Exit(u8),
}

impl CliResult {
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::GenericFailure => 1,
            Self::ValidationFailed => 2,
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
            _ => Self::GenericFailure,
        }
    }
}

/// JSON contract version emitted on every structured response. Bumping
/// it is a breaking change for skill authors. v3 fixes the `created-baseline`
/// `MergeOp` discriminant (was `created_baseline` in v2).
pub const JSON_SCHEMA_VERSION: u64 = 3;

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
    let envelope = JsonEnvelope::wrap(payload);
    let body = serde_json::to_string_pretty(&envelope)
        .map_err(|err| Error::Config(format!("failed to serialize JSON response: {err}")))?;
    println!("{body}");
    Ok(())
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
