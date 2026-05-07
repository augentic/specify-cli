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
            Error::SpecifyVersionTooOld { .. } => Self::VersionTooOld,
            Error::Validation { .. }
            | Error::ToolPermissionDenied(_)
            | Error::ToolNotDeclared { .. } => Self::ValidationFailed,
            _ => Self::GenericFailure,
        }
    }
}

/// JSON contract version emitted on every structured response. Bumping
/// this field is a breaking change for skill authors — see RFC-1
/// §"JSON Contract Versioning".
///
/// # v1 → v2 diff (RFC-2 §2)
///
/// - Every JSON key is now kebab-case. `schema_version` → `schema-version`,
///   `change_dir` → `change-dir`, `defined_at` → `defined-at`, and so on for
///   every snake-case key that was ever emitted by the CLI (see RFC-2 §2.1
///   for the full rename table). Library-derived types were already kebab
///   via `#[serde(rename_all = "kebab-case")]`; v2 aligns the hand-built
///   `json!({...})` blocks in `src/main.rs` and the
///   `specify-validate::serialize_report` helper with the same rule.
/// - New read verb `specify change outcome <name>` (added in RFC-2 §1.1 /
///   L0.A1) shipped under the v2 contract.
/// - Error variant identifiers surfaced as the `"error"` value in failure
///   payloads are kebab-case too. `Error::variant_str()` is the canonical
///   source for these stable identifiers.
/// - No shape changes beyond the casing: key sets, nesting, and value
///   types are frozen.
pub const JSON_SCHEMA_VERSION: u64 = 2;

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

pub fn emit_response<T: Serialize>(payload: T) {
    let envelope = JsonEnvelope::wrap(payload);
    println!("{}", serde_json::to_string_pretty(&envelope).expect("JSON serialise"));
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
        emit_response(ValidationErrorResponse {
            error: "validation".to_string(),
            message: err.to_string(),
            exit_code: code.code(),
            results: results.iter().map(validation_result_response).collect(),
        });
        return;
    }

    let variant = err.variant_str();
    emit_response(ErrorResponse {
        error: variant.to_string(),
        message: err.to_string(),
        exit_code: code.code(),
    });
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
