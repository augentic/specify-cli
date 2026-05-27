//! JSON-Schema validation helpers shared across crates.
//!
//! [`compile_schema`] parses and compiles an embedded schema string.
//! [`validate_value`] runs a compiled validator over a `serde_json`
//! instance and returns the unified [`ValidationSummary`] shape
//! callers fold into the appropriate [`Error`] variant.
//! [`validate_serialisable`] serialises any [`Serialize`] value and
//! runs the same check; [`read_yaml_as_json`] is the YAML-to-JSON
//! bridge used by file-driven validators.

use std::fs;
use std::path::Path;

use jsonschema::Validator;
use jsonschema::error::{ValidationError, ValidationErrorKind};
use serde::Serialize;
use serde_json::Value as JsonValue;
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};

/// Read `path` as UTF-8 YAML and reinterpret the document as a
/// [`serde_json::Value`] for schema validation. Returns a free-form
/// error string so callers can attach their own provenance.
///
/// # Errors
///
/// Returns the underlying I/O or YAML parse message; the caller wraps
/// it into an [`Error`] variant whose `detail` carries the originating
/// path.
pub fn read_yaml_as_json(path: &Path) -> std::result::Result<JsonValue, String> {
    let raw = fs::read_to_string(path).map_err(|err| format!("read failed: {err}"))?;
    serde_saphyr::from_str(&raw).map_err(|err| format!("YAML parse failed: {err}"))
}

/// Serialise `value` to JSON and validate against `schema_source`.
///
/// Returns `Ok(())` on a clean validation; otherwise an
/// [`Error::Validation`] whose [`ValidationSummary`] entries carry
/// `rule_id` and `rule`.
///
/// # Errors
///
/// - [`Error::Diag`] when `value` is not JSON-serialisable.
/// - [`Error::Validation`] when the instance fails the schema.
pub fn validate_serialisable<T: Serialize>(
    value: &T, schema_source: &str, rule_id: &str, rule: &str, serialise_code: &'static str,
    serialise_label: &str,
) -> Result<(), Error> {
    let instance = serde_json::to_value(value).map_err(|err| Error::Diag {
        code: serialise_code,
        detail: format!(
            "failed to serialise {serialise_label} to JSON for schema validation: {err}"
        ),
    })?;
    let failures: Vec<ValidationSummary> = validate_value(&instance, schema_source, rule_id, rule)
        .into_iter()
        .filter(|summary| summary.status == ValidationStatus::Fail)
        .collect();
    if failures.is_empty() { Ok(()) } else { Err(Error::Validation { results: failures }) }
}

/// Validate `instance` against the embedded JSON Schema `schema_source`.
///
/// Returns one `Pass`-status [`ValidationSummary`] entry on a clean
/// validation, one `Fail` entry with the joined error list on a schema
/// mismatch, or a single `Fail` carrying the meta-failure reason if the
/// embedded schema itself cannot be parsed or compiled. Callers wrap
/// the resulting vector into the [`Error`] variant that suits their
/// exit-code policy: structural manifest checks fold failures into
/// [`Error::Diag`] (exit 1); plan / evidence checks fold into
/// [`Error::Validation`] (exit 2).
#[must_use]
pub fn validate_value(
    instance: &JsonValue, schema_source: &str, rule_id: &str, rule: &str,
) -> Vec<ValidationSummary> {
    let validator = match compile_schema(schema_source) {
        Ok(v) => v,
        Err(err) => {
            return vec![ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: rule_id.into(),
                rule: rule.into(),
                detail: Some(err.to_string()),
            }];
        }
    };
    let errors: Vec<String> =
        validator.iter_errors(instance).map(|err| validation_error_detail(&err)).collect();
    if errors.is_empty() {
        vec![ValidationSummary {
            status: ValidationStatus::Pass,
            rule_id: rule_id.into(),
            rule: rule.into(),
            detail: None,
        }]
    } else {
        vec![ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: rule_id.into(),
            rule: rule.into(),
            detail: Some(errors.join("; ")),
        }]
    }
}

/// Parse and compile an embedded JSON Schema string.
///
/// # Errors
///
/// Returns [`Error::Diag`] with `schema-meta-loadable` if the schema
/// source is not valid JSON, or `schema-meta-compilable` if the JSON
/// is not a valid JSON Schema.
pub fn compile_schema(schema_source: &str) -> Result<Validator> {
    let schema: JsonValue = serde_json::from_str(schema_source).map_err(|err| Error::Diag {
        code: "schema-meta-loadable",
        detail: format!("embedded JSON Schema does not parse as JSON: {err}"),
    })?;
    jsonschema::validator_for(&schema).map_err(|err| Error::Diag {
        code: "schema-meta-compilable",
        detail: format!("embedded JSON Schema does not compile: {err}"),
    })
}

pub(crate) fn validation_error_detail(err: &ValidationError<'_>) -> String {
    let path = match err.kind() {
        ValidationErrorKind::AdditionalProperties { unexpected } if unexpected.len() == 1 => {
            child_pointer(&err.instance_path().to_string(), &unexpected[0])
        }
        _ => err.instance_path().to_string(),
    };
    format!("{path}: {err}")
}

pub(crate) fn child_pointer(parent: &str, property: &str) -> String {
    let property = property.replace('~', "~0").replace('/', "~1");
    if parent.is_empty() { format!("/{property}") } else { format!("{parent}/{property}") }
}
