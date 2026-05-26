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
use jsonschema::error::{ValidationError, ValidationErrorKind};
use serde::Serialize;
use serde_json::Value as JsonValue;
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};

use crate::change::Plan;

const PLAN_JSON_SCHEMA: &str = include_str!("../../../schemas/plan/plan.schema.json");
const EVIDENCE_JSON_SCHEMA: &str = include_str!("../../../schemas/evidence.schema.json");
pub(crate) const FUSION_JSON_SCHEMA: &str =
    include_str!("../../../schemas/slice/fusion.schema.json");
const COMPONENTS_JSON_SCHEMA: &str =
    include_str!("../../../schemas/design-system/components.schema.json");
// RFC-28 Phase 2 runtime codex and review schemas. Visible across the
// crate in test builds so the `codex` module can validate its DTO
// round-trips against the same wire schemas the resolver and finding
// validators will consume in CH-12+/CH-16; CH-16 will un-gate these
// when it lands the public `validate_*` helpers.
#[cfg(test)]
pub(crate) const RESOLVED_CODEX_JSON_SCHEMA: &str =
    include_str!("../../../schemas/codex/resolved.schema.json");
#[cfg(test)]
pub(crate) const CODEX_RULE_JSON_SCHEMA: &str =
    include_str!("../../../schemas/codex/codex-rule.schema.json");
#[cfg(test)]
pub(crate) const REVIEW_FINDING_JSON_SCHEMA: &str =
    include_str!("../../../schemas/review/finding.schema.json");

/// Validate `plan` against the embedded `schemas/plan/plan.schema.json`.
///
/// Returns `Ok(())` on a clean validation; otherwise an
/// [`Error::Validation`] whose single [`ValidationSummary`] carries the
/// stable `rule_id` `"plan-schema"` and the JSON-pointer + reason list
/// the schema produced. Used by `specrun plan add` and `specrun plan
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
    validate_serialisable(
        plan,
        PLAN_JSON_SCHEMA,
        "plan-schema",
        "plan.yaml conforms to schemas/plan/plan.schema.json",
        "plan-schema-serialise",
        "plan",
    )
}

/// Validate raw `plan.yaml` content before typed deserialisation.
///
/// # Errors
///
/// Returns [`Error::Validation`] when YAML parsing or schema validation fails.
pub fn validate_plan_yaml(content: &str) -> Result<()> {
    let instance = serde_saphyr::from_str(content).map_err(|err| {
        Error::validation_failed(
            "plan-schema",
            "plan.yaml conforms to schemas/plan/plan.schema.json",
            format!("YAML parse failed: {err}"),
        )
    })?;
    err_from_failures(validation_failures(
        &instance,
        PLAN_JSON_SCHEMA,
        "plan-schema",
        "plan.yaml conforms to schemas/plan/plan.schema.json",
    ))
}

/// Validate raw `plan.yaml` before typed deserialisation.
///
/// # Errors
///
/// Returns [`Error::Validation`] when YAML parsing or schema validation fails.
pub fn validate_plan_file(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path).map_err(|err| {
        Error::validation_failed(
            "plan-schema",
            "plan.yaml conforms to schemas/plan/plan.schema.json",
            format!("read failed: {err}"),
        )
    })?;
    validate_plan_yaml(&content)
}

/// Sorted paths to `.yaml`/`.yml` files under `<slice_dir>/evidence/`.
///
/// The walk is non-recursive: only direct children of `evidence/` whose
/// extension is `yaml` or `yml` are considered. Returns an empty
/// vector when `evidence/` is missing or not a directory.
///
/// # Errors
///
/// - [`Error::Filesystem`] if `evidence/` exists but cannot be read.
pub fn evidence_yaml_paths(slice_dir: &Path) -> Result<Vec<PathBuf>> {
    let evidence_dir = slice_dir.join("evidence");
    if !evidence_dir.is_dir() {
        return Ok(Vec::new());
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
    Ok(paths)
}

/// Validate every `*.yaml` file under `<slice_dir>/evidence/` against
/// the embedded `schemas/evidence.schema.json`.
///
/// `slice_dir` is the directory typically at
/// `.specify/slices/<name>/`. The evidence subdirectory is optional —
/// returning `Ok(())` when it is absent matches the workflow §Extraction
/// reliability rule that an empty `claims: []` (or no Evidence at all
/// before extract runs) is valid.
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
    let paths = evidence_yaml_paths(slice_dir)?;

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

/// Validate raw `components.yaml` content against the embedded
/// `schemas/design-system/components.schema.json`.
///
/// `source_path` labels error messages with the originating file.
///
/// # Errors
///
/// Returns [`Error::Validation`] when YAML parsing or schema validation fails.
pub fn validate_components_yaml(content: &str, source_path: &Path) -> Result<()> {
    let instance: JsonValue = serde_saphyr::from_str(content).map_err(|err| {
        Error::validation_failed(
            "catalog-schema",
            "components.yaml conforms to schemas/design-system/components.schema.json",
            format!("{}: YAML parse failed: {err}", source_path.display()),
        )
    })?;
    err_from_failures(validation_failures(
        &instance,
        COMPONENTS_JSON_SCHEMA,
        "catalog-schema",
        "components.yaml conforms to schemas/design-system/components.schema.json",
    ))
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
    err_from_failures(validation_failures(&instance, schema_source, rule_id, rule))
}

fn validation_failures(
    instance: &JsonValue, schema_source: &str, rule_id: &str, rule: &str,
) -> Vec<ValidationSummary> {
    validate_value(instance, schema_source, rule_id, rule)
        .into_iter()
        .filter(|summary| summary.status == ValidationStatus::Fail)
        .collect()
}

fn err_from_failures(results: Vec<ValidationSummary>) -> Result<()> {
    if results.is_empty() { Ok(()) } else { Err(Error::Validation { results }) }
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

fn validation_error_detail(err: &ValidationError<'_>) -> String {
    let path = match err.kind() {
        ValidationErrorKind::AdditionalProperties { unexpected } if unexpected.len() == 1 => {
            child_pointer(&err.instance_path().to_string(), &unexpected[0])
        }
        _ => err.instance_path().to_string(),
    };
    format!("{path}: {err}")
}

fn child_pointer(parent: &str, property: &str) -> String {
    let property = property.replace('~', "~0").replace('/', "~1");
    if parent.is_empty() { format!("/{property}") } else { format!("{parent}/{property}") }
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

    /// Embedded fusion schema parses and compiles.
    #[test]
    fn fusion_schema_compiles() {
        compile_schema(FUSION_JSON_SCHEMA).expect("fusion schema compiles");
    }

    /// Embedded components catalog schema parses and compiles.
    #[test]
    fn components_schema_compiles() {
        compile_schema(COMPONENTS_JSON_SCHEMA).expect("components schema compiles");
    }

    /// Embedded resolved codex export schema parses and compiles
    /// (RFC-28 §Resolved codex export).
    #[test]
    fn resolved_codex_schema_compiles() {
        compile_schema(RESOLVED_CODEX_JSON_SCHEMA).expect("resolved codex schema compiles");
    }

    /// Embedded vendored codex-rule schema parses and compiles. The
    /// runtime copy is byte-identical to
    /// `crates/authoring/schemas/codex-rule.schema.json`; the
    /// `codex.schema-drift` predicate (CH-09) enforces parity.
    #[test]
    fn codex_rule_schema_compiles() {
        compile_schema(CODEX_RULE_JSON_SCHEMA).expect("codex-rule schema compiles");
    }

    /// Embedded `ReviewFinding` schema parses and compiles
    /// (RFC-28 §Structured review finding schema).
    #[test]
    fn review_finding_schema_compiles() {
        compile_schema(REVIEW_FINDING_JSON_SCHEMA).expect("review finding schema compiles");
    }

    /// The `UNI-014` example from RFC-28 §Resolved codex export
    /// validates cleanly against the resolved-codex schema.
    #[test]
    fn resolved_codex_schema_accepts_rfc_example() {
        let instance = serde_json::json!({
            "version": 1,
            "target-adapter": "omnia",
            "source-adapters": ["code-typescript"],
            "rules": [
                {
                    "rule-id": "UNI-014",
                    "title": "Hardcoded Configuration",
                    "severity": "important",
                    "trigger": "Generated code embeds environment-specific configuration instead of routing it through declared configuration.",
                    "review-mode": "hybrid",
                    "origin": "shared",
                    "path-root": "codex-root",
                    "path": "adapters/shared/codex/universal/hardcoded-configuration.md",
                    "applicability": {
                        "adapters": ["omnia"],
                        "languages": ["rust"],
                        "artifacts": ["code"]
                    },
                    "deterministic-hints": [
                        {
                            "kind": "regex",
                            "value": "https?://",
                            "description": "Literal URL in generated code."
                        }
                    ],
                    "references": [
                        {
                            "label": "Omnia guardrails",
                            "path": "adapters/targets/omnia/references/guardrails.md"
                        }
                    ],
                    "body": "## Rule\n\nConfiguration values that vary between deployments must not be hardcoded in generated code.\n",
                    "deprecated": null
                }
            ]
        });
        let validator =
            compile_schema(RESOLVED_CODEX_JSON_SCHEMA).expect("resolved codex schema compiles");
        let errors: Vec<String> =
            validator.iter_errors(&instance).map(|e| validation_error_detail(&e)).collect();
        assert!(errors.is_empty(), "RFC-28 UNI-014 example must validate; errors: {errors:?}");
    }

    /// The `FIND-0001` example from RFC-28 §Structured review finding
    /// schema validates cleanly against the finding schema. The
    /// fingerprint placeholder `sha256:...` from the RFC body is
    /// replaced with a deterministic 64-hex-char digest so the
    /// fingerprint pattern check passes.
    #[test]
    fn review_finding_schema_accepts_rfc_example() {
        let instance = serde_json::json!({
            "id": "FIND-0001",
            "rule-id": "UNI-014",
            "title": "Literal deployment URL in generated handler",
            "severity": "important",
            "source": "hybrid",
            "target-adapter": "omnia",
            "slice": "billing-invoice-export",
            "artifact": "code",
            "location": {
                "path": "crates/invoice_export/src/config.rs",
                "line": 18
            },
            "evidence": {
                "kind": "snippet",
                "value": "const BASE_URL: &str = \"https://api.example.com\";"
            },
            "impact": "Generated code will point every deployment at the same external endpoint.",
            "remediation": "Read the endpoint from Omnia configuration and add a required config key to the design.",
            "confidence": "high",
            "fingerprint": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        });
        let validator =
            compile_schema(REVIEW_FINDING_JSON_SCHEMA).expect("review finding schema compiles");
        let errors: Vec<String> =
            validator.iter_errors(&instance).map(|e| validation_error_detail(&e)).collect();
        assert!(errors.is_empty(), "RFC-28 FIND-0001 example must validate; errors: {errors:?}");
    }

    /// The codex-rule frontmatter example in RFC-28 §Codex file shape
    /// validates cleanly against the vendored codex-rule schema. This
    /// pairs the frontmatter block (in YAML in the RFC) with its JSON
    /// equivalent, so the runtime schema gets the same structural
    /// coverage as the authoring schema.
    #[test]
    fn codex_rule_schema_accepts_rfc_example() {
        let instance = serde_json::json!({
            "id": "UNI-014",
            "title": "Hardcoded Configuration",
            "severity": "important",
            "trigger": "Generated code embeds environment-specific configuration instead of routing it through declared configuration.",
            "applicability": {
                "adapters": ["omnia"],
                "languages": ["rust"],
                "artifacts": ["code"]
            },
            "review_mode": "hybrid",
            "deterministic_hints": [
                {
                    "kind": "regex",
                    "value": "https?://",
                    "description": "Literal URL in generated code."
                }
            ]
        });
        let validator = compile_schema(CODEX_RULE_JSON_SCHEMA).expect("codex-rule schema compiles");
        let errors: Vec<String> =
            validator.iter_errors(&instance).map(|e| validation_error_detail(&e)).collect();
        assert!(errors.is_empty(), "RFC-28 UNI-014 frontmatter must validate; errors: {errors:?}");
    }

    /// An empty evidence directory (or missing one) passes — empty
    /// extraction is a legal slice state per workflow §Extraction
    /// reliability.
    #[test]
    fn missing_evidence_dir_is_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        validate_evidence_dir(dir.path()).expect("missing evidence dir is ok");
    }
}
