use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use specify::Error;

use crate::cli::OutputFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliResult {
    Success,
    GenericFailure,
    ValidationFailed,
    VersionTooOld,
}

impl CliResult {
    pub fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::GenericFailure => 1,
            Self::ValidationFailed => 2,
            Self::VersionTooOld => 3,
        }
    }
}

impl From<CliResult> for ExitCode {
    fn from(r: CliResult) -> Self {
        ExitCode::from(r.code())
    }
}

impl From<&Error> for CliResult {
    fn from(err: &Error) -> Self {
        match err {
            Error::SpecifyVersionTooOld { .. } => CliResult::VersionTooOld,
            Error::Validation { .. } => CliResult::ValidationFailed,
            _ => CliResult::GenericFailure,
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
///   payloads are kebab-case too: `not_initialized` → `not-initialized`,
///   `schema_resolution` → `schema-resolution`, `specify_version_too_old`
///   → `specify-version-too-old`, `plan_transition` → `plan-transition`,
///   `plan_has_outstanding_work` → `plan-has-outstanding-work`, and
///   `driver_busy` → `driver-busy`. Single-word variants (`io`, `yaml`,
///   `config`, `merge`, `lifecycle`, `validation`) were already kebab-safe
///   and are unchanged.
/// - No shape changes beyond the casing: key sets, nesting, and value
///   types are frozen.
pub(crate) const JSON_SCHEMA_VERSION: u64 = 2;

pub(crate) fn emit_error(format: OutputFormat, err: &Error) -> CliResult {
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
pub(crate) struct JsonEnvelope<T: Serialize> {
    #[serde(rename = "schema-version")]
    schema_version: u64,
    #[serde(flatten)]
    payload: T,
}

impl<T: Serialize> JsonEnvelope<T> {
    fn wrap(payload: T) -> Self {
        Self {
            schema_version: JSON_SCHEMA_VERSION,
            payload,
        }
    }
}

pub(crate) fn emit_response<T: Serialize>(payload: T) {
    let envelope = JsonEnvelope::wrap(payload);
    let value = serde_json::to_value(&envelope).expect("Serialize to Value");
    println!("{}", serde_json::to_string_pretty(&value).expect("JSON serialise"));
}

/// Serialise a JSON payload with `schema-version` automatically set on
/// object-shaped responses.
pub(crate) fn emit_json(value: serde_json::Value) {
    let wrapped = match value {
        serde_json::Value::Object(mut map) => {
            map.entry("schema-version".to_string())
                .or_insert(serde_json::Value::from(JSON_SCHEMA_VERSION));
            serde_json::Value::Object(map)
        }
        other => other,
    };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("JSON serialise"));
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ErrorResponse {
    pub error: String,
    pub message: String,
    pub exit_code: u8,
}

pub(crate) fn emit_json_error(err: &Error, code: CliResult) {
    let variant = match err {
        Error::NotInitialized => "not-initialized",
        Error::SchemaResolution(_) => "schema-resolution",
        Error::Config(_) => "config",
        Error::Validation { .. } => "validation",
        Error::Merge(_) => "merge",
        Error::Lifecycle { .. } => "lifecycle",
        Error::SpecifyVersionTooOld { .. } => "specify-version-too-old",
        Error::PlanTransition { .. } => "plan-transition",
        Error::PlanHasOutstandingWork { .. } => "plan-has-outstanding-work",
        Error::DriverBusy { .. } => "driver-busy",
        Error::ArtifactNotFound { .. } => "artifact-not-found",
        Error::InvalidName(_) => "invalid-name",
        Error::Io(_) => "io",
        Error::Yaml(_) => "yaml",
            _ => unreachable!(),
    };
    emit_response(ErrorResponse {
        error: variant.to_string(),
        message: err.to_string(),
        exit_code: code.code(),
    });
}

pub(crate) fn absolute_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
