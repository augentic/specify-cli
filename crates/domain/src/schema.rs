//! JSON Schema validation hooks for RFC-25 on-disk artifacts.
//!
//! Covers `plan.yaml` (refined for structured `slices[].sources[]`
//! bindings, the `target` field, and the slice-level `divergence` enum)
//! and per-source `Evidence` files under `.specify/slices/<name>/evidence/`.
//!
//! Schemas are embedded at compile time via `include_str!` so the binary
//! carries them with no runtime filesystem lookup. The validators
//! return [`Error::Validation`] on a schema mismatch so the CLI exits
//! with code 2 (`Exit::ValidationFailed` in the binary crate).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use jsonschema::Validator;
use serde_json::Value as JsonValue;
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};

use crate::change::Plan;

const PLAN_JSON_SCHEMA: &str = include_str!("../../../schemas/plan/plan.schema.json");
const EVIDENCE_JSON_SCHEMA: &str = include_str!("../../../schemas/evidence.schema.json");

/// Validate `plan` against the embedded `schemas/plan/plan.schema.json`.
///
/// Returns `Ok(())` on a clean validation; otherwise an
/// [`Error::Validation`] whose single [`ValidationSummary`] carries the
/// stable `rule_id` `"plan-schema"` and the JSON-pointer + reason list
/// the schema produced. Used by `specify plan add` and `specify plan
/// amend` so first-use validation refuses to write a malformed plan.
///
/// # Errors
///
/// Returns [`Error::Validation`] when the in-memory plan fails the
/// schema; falls back to [`Error::Diag`] when the embedded schema is
/// unparseable or the plan is not JSON-serialisable (both should be
/// unreachable in production — they exist to surface a corrupted
/// binary).
pub fn validate_plan(plan: &Plan) -> Result<()> {
    let instance = serde_json::to_value(plan).map_err(|err| Error::Diag {
        code: "plan-schema-serialise",
        detail: format!("failed to serialise plan to JSON for schema validation: {err}"),
    })?;
    let results: Vec<ValidationSummary> = validate_value(
        &instance,
        PLAN_JSON_SCHEMA,
        "plan-schema",
        "plan.yaml conforms to schemas/plan/plan.schema.json",
    )
    .into_iter()
    .filter(|s| s.status == ValidationStatus::Fail)
    .collect();
    if results.is_empty() { Ok(()) } else { Err(Error::Validation { results }) }
}

/// Validate every `*.yaml` file under `<slice_dir>/evidence/` against
/// the embedded `schemas/evidence.schema.json`.
///
/// `slice_dir` is the directory typically at
/// `.specify/slices/<name>/`. The evidence subdirectory is optional —
/// returning `Ok(())` when it is absent matches the RFC-25 §Extraction
/// reliability rule that an empty `claims: []` (or no Evidence at all
/// before extract runs) is valid. The walk is non-recursive: only
/// direct children of `evidence/` whose extension is `yaml` or `yml`
/// are considered.
///
/// All findings are aggregated and returned in a single
/// [`Error::Validation`] so the caller sees every malformed file in
/// one pass.
///
/// # Errors
///
/// - [`Error::Filesystem`] if `evidence/` exists but cannot be read.
/// - [`Error::Validation`] if any Evidence file fails YAML parse or
///   schema validation.
pub fn validate_evidence_dir(slice_dir: &Path) -> Result<()> {
    let evidence_dir = slice_dir.join("evidence");
    if !evidence_dir.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(&evidence_dir).map_err(|source| Error::Filesystem {
        op: "readdir",
        path: evidence_dir.clone(),
        source,
    })?;

    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Filesystem {
            op: "readdir-entry",
            path: evidence_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");
        if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut summaries: Vec<ValidationSummary> = Vec::new();
    for path in &paths {
        match read_yaml_as_json(path) {
            Ok(instance) => {
                for summary in validate_value(
                    &instance,
                    EVIDENCE_JSON_SCHEMA,
                    "evidence-schema",
                    "evidence file conforms to schemas/evidence.schema.json",
                ) {
                    if summary.status == ValidationStatus::Fail {
                        summaries.push(relabel_with_path(summary, path));
                    }
                }
            }
            Err(err) => {
                summaries.push(ValidationSummary {
                    status: ValidationStatus::Fail,
                    rule_id: "evidence-schema".into(),
                    rule: "evidence file conforms to schemas/evidence.schema.json".into(),
                    detail: Some(format!("{}: {err}", path.display())),
                });
            }
        }
    }

    if summaries.is_empty() { Ok(()) } else { Err(Error::Validation { results: summaries }) }
}

fn read_yaml_as_json(path: &Path) -> std::result::Result<JsonValue, String> {
    let raw = fs::read_to_string(path).map_err(|err| format!("read failed: {err}"))?;
    serde_saphyr::from_str(&raw).map_err(|err| format!("YAML parse failed: {err}"))
}

fn relabel_with_path(mut summary: ValidationSummary, path: &Path) -> ValidationSummary {
    let detail = summary.detail.take().unwrap_or_default();
    summary.detail = Some(if detail.is_empty() {
        path.display().to_string()
    } else {
        format!("{}: {}", path.display(), detail)
    });
    summary
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
        validator.iter_errors(instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();
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

fn compile_schema(schema_source: &str) -> Result<Validator> {
    let schema: JsonValue = serde_json::from_str(schema_source).map_err(|err| Error::Diag {
        code: "schema-meta-loadable",
        detail: format!("embedded JSON Schema does not parse as JSON: {err}"),
    })?;
    jsonschema::validator_for(&schema).map_err(|err| Error::Diag {
        code: "schema-meta-compilable",
        detail: format!("embedded JSON Schema does not compile: {err}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Embedded plan schema parses and compiles. Cheap smoke that
    /// catches a corrupted `include_str!` import.
    #[test]
    fn plan_schema_compiles() {
        compile_schema(PLAN_JSON_SCHEMA).expect("plan schema compiles");
    }

    /// Embedded evidence schema parses and compiles.
    #[test]
    fn evidence_schema_compiles() {
        compile_schema(EVIDENCE_JSON_SCHEMA).expect("evidence schema compiles");
    }

    /// An empty evidence directory (or missing one) passes — empty
    /// extraction is a legal slice state per RFC-25 §Extraction
    /// reliability.
    #[test]
    fn missing_evidence_dir_is_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        validate_evidence_dir(dir.path()).expect("missing evidence dir is ok");
    }
}
