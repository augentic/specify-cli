//! Domain-shaped JSON Schema validation hooks for source/target adapter split on-disk
//! artifacts.
//!
//! The raw JSON-Schema plumbing and embedded constants live in
//! [`specify_schema`] per [DECISIONS.md § Standards layer split into `specify-standards` and `specify-schema`](../../DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema); this module holds
//! the workflow-aware wrappers — they import [`crate::change::Plan`],
//! aggregate per-file findings into a single
//! [`specify_error::Error::Validation`] payload, and pin the wire
//! `rule_id` strings the CLI surfaces.
//!
//! Schemas are embedded by [`specify_schema::constants`] via
//! `include_str!`. The validators return [`Error::Validation`] on a
//! schema mismatch so the CLI exits with code 2
//! (`Exit::ValidationFailed` in the binary crate).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use specify_error::{Error, Result};
pub use specify_schema::{
    COMPONENTS_JSON_SCHEMA, DIAGNOSTIC_JSON_SCHEMA, EVIDENCE_JSON_SCHEMA, PLAN_JSON_SCHEMA,
    PROVENANCE_JSON_SCHEMA, RESOLVED_RULES_JSON_SCHEMA, RULE_JSON_SCHEMA, compile_schema,
    read_yaml_as_json, validate_serialisable, validate_value,
};
use specify_schema::{ValidationStatus, ValidationSummary, join_details};

use crate::change::Plan;

/// Validate `plan` against the embedded `schemas/plan/plan.schema.json`.
///
/// Returns `Ok(())` on a clean validation; otherwise a payload-free
/// [`Error::Validation`] keyed on the code `"plan-schema"`, with the
/// JSON-pointer + reason list the schema produced joined into the
/// detail. Used by `specrun plan add` and `specrun plan amend` so
/// first-use validation refuses to write a malformed plan.
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
    err_from_failures(
        "plan-schema",
        &validation_failures(
            &instance,
            PLAN_JSON_SCHEMA,
            "plan-schema",
            "plan.yaml conforms to schemas/plan/plan.schema.json",
        ),
    )
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

    if summaries.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation {
            code: "evidence-schema".to_string(),
            detail: join_details(&summaries),
        })
    }
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
    err_from_failures(
        "catalog-schema",
        &validation_failures(
            &instance,
            COMPONENTS_JSON_SCHEMA,
            "catalog-schema",
            "components.yaml conforms to schemas/design-system/components.schema.json",
        ),
    )
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

fn validation_failures(
    instance: &JsonValue, schema_source: &str, rule_id: &str, rule: &str,
) -> Vec<ValidationSummary> {
    validate_value(instance, schema_source, rule_id, rule)
        .into_iter()
        .filter(|summary| summary.status == ValidationStatus::Fail)
        .collect()
}

fn err_from_failures(code: &str, results: &[ValidationSummary]) -> Result<()> {
    if results.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation {
            code: code.to_string(),
            detail: join_details(results),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `UNI-014` example for the `ResolvedRules` export
    /// validates cleanly against the resolved-codex schema.
    #[test]
    fn resolved_codex_schema_accepts_contract_example() {
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
                    "lint-mode": "hybrid",
                    "origin": "shared",
                    "path-root": "rules-root",
                    "path": "adapters/shared/rules/universal/hardcoded-configuration.md",
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
            compile_schema(RESOLVED_RULES_JSON_SCHEMA).expect("resolved codex schema compiles");
        let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "UNI-014 example must validate; errors: {errors:?}");
    }

    /// The `FIND-0001` example for structured lint findings
    /// schema validates cleanly against the finding schema. The
    /// fingerprint placeholder `sha256:...` from the contract is
    /// replaced with a deterministic 64-hex-char digest so the
    /// fingerprint pattern check passes.
    #[test]
    fn review_finding_schema_accepts_contract_example() {
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
            compile_schema(DIAGNOSTIC_JSON_SCHEMA).expect("review finding schema compiles");
        let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "FIND-0001 example must validate; errors: {errors:?}");
    }

    /// The rule frontmatter example for codex file shape
    /// validates cleanly against the vendored codex-rule schema.
    #[test]
    fn codex_rule_schema_accepts_contract_example() {
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
            "lint_mode": "hybrid",
            "deterministic_hints": [
                {
                    "kind": "regex",
                    "value": "https?://",
                    "description": "Literal URL in generated code."
                }
            ]
        });
        let validator = compile_schema(RULE_JSON_SCHEMA).expect("codex-rule schema compiles");
        let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "UNI-014 frontmatter must validate; errors: {errors:?}");
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
