//! `kind: schema` evaluator per `kind: schema` evaluator contract.
//!
//! `hint.value` selects one of two shapes:
//!
//! - **Registered schema id** (bare token; e.g. `codex-rule`) — looks
//!   up an embedded schema [`specify_schema`] ships with the CLI.
//! - **Project-relative `$ref`** (`./...` or `../...`) — reads the
//!   schema file from the project tree, after refusing paths that
//!   escape `project_dir` via `..`.
//!
//! `http(s)://` references are rejected so `specrun lint` runs
//! offline and reproducibly.
//!
//! Per-file targeting follows the v1 default — the hint applies to
//! the parsed body of the candidate file. JSON / YAML / TOML files
//! are parsed; markdown files fall back to their extracted
//! frontmatter via [`crate::lint::WorkspaceModel::frontmatter`].
//! The `target: frontmatter` extension from the contract is reserved
//! — the closed [`crate::rules::DeterministicHint`] shape carries no
//! `target` field, so v1 cannot opt into it.
//!
//! Each `iter_errors` entry maps to one [`crate::rules::Diagnostic`]
//! with `Structured` evidence carrying the failing JSON-pointer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use jsonschema::Validator;
use serde_json::Value;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};
use specify_schema::{
    COMPONENTS_JSON_SCHEMA, DIAGNOSTIC_JSON_SCHEMA, DIAGNOSTIC_REPORT_JSON_SCHEMA,
    EVIDENCE_JSON_SCHEMA, PLAN_JSON_SCHEMA, PROVENANCE_JSON_SCHEMA, RESOLVED_RULES_JSON_SCHEMA,
    RULE_JSON_SCHEMA, WORKSPACE_MODEL_JSON_SCHEMA, compile_schema,
};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{DeterministicHint, ResolvedRule};

static REGISTERED_SCHEMAS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("rule", RULE_JSON_SCHEMA),
        ("resolved-rules", RESOLVED_RULES_JSON_SCHEMA),
        ("review-finding", DIAGNOSTIC_JSON_SCHEMA),
        ("review-result", DIAGNOSTIC_REPORT_JSON_SCHEMA),
        ("workspace-model", WORKSPACE_MODEL_JSON_SCHEMA),
        ("plan", PLAN_JSON_SCHEMA),
        ("evidence", EVIDENCE_JSON_SCHEMA),
        ("provenance", PROVENANCE_JSON_SCHEMA),
        ("components", COMPONENTS_JSON_SCHEMA),
    ])
});

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], project_dir: &Path,
    model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let validator = compile_schema_for_hint(rule, hint, project_dir)?;

    let mut out: Vec<Diagnostic> = Vec::new();
    for candidate in candidates {
        let candidate_str = candidate.to_string_lossy().into_owned();
        let Some(instance) =
            load_candidate_instance(model, project_dir, candidate, &candidate_str)?
        else {
            continue;
        };
        for error in validator.iter_errors(&instance) {
            let pointer = error.instance_path().to_string();
            let evidence = FindingEvidence::Structured {
                summary: error.to_string(),
                data: serde_json::json!({ "json_pointer": pointer }),
                locations: None,
            };
            let location = FindingLocation {
                path: candidate_str.clone(),
                line: None,
                column: None,
                end_line: None,
                end_column: None,
            };
            let title = format!("{}: schema validation failed", rule.title);
            let finding = make_finding(rule, *next_id, title, Some(location), evidence);
            *next_id += 1;
            out.push(finding);
        }
    }
    Ok(out)
}

fn compile_schema_for_hint(
    rule: &ResolvedRule, hint: &DeterministicHint, project_dir: &Path,
) -> Result<Validator, HintError> {
    let raw = hint.value.trim();
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Err(HintError::SchemaResolve {
            rule_id: rule.rule_id.clone(),
            schema_ref: raw.to_owned(),
            reason: "external http(s) references are not allowed; specrun lint must run offline"
                .to_owned(),
        });
    }
    if raw.starts_with("./") || raw.starts_with("../") {
        let resolved = resolve_project_relative(project_dir, raw).map_err(|reason| {
            HintError::SchemaResolve {
                rule_id: rule.rule_id.clone(),
                schema_ref: raw.to_owned(),
                reason,
            }
        })?;
        let body = std::fs::read_to_string(&resolved).map_err(|err| HintError::Filesystem {
            op: "read-schema",
            path: resolved.clone(),
            source: err,
        })?;
        return compile_schema(&body).map_err(|err| HintError::SchemaCompile {
            rule_id: rule.rule_id.clone(),
            schema_ref: raw.to_owned(),
            detail: err.to_string(),
        });
    }
    let registered = REGISTERED_SCHEMAS.get(raw).ok_or_else(|| HintError::SchemaResolve {
        rule_id: rule.rule_id.clone(),
        schema_ref: raw.to_owned(),
        reason: "unknown registered schema id".to_owned(),
    })?;
    compile_schema(registered).map_err(|err| HintError::SchemaCompile {
        rule_id: rule.rule_id.clone(),
        schema_ref: raw.to_owned(),
        detail: err.to_string(),
    })
}

fn resolve_project_relative(project_dir: &Path, raw: &str) -> Result<PathBuf, String> {
    let candidate = project_dir.join(raw);
    let normalised = normalise(&candidate);
    if !normalised.starts_with(project_dir) {
        return Err("schema reference escapes project directory".to_owned());
    }
    Ok(normalised)
}

fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn load_candidate_instance(
    model: &WorkspaceModel, project_dir: &Path, candidate: &Path, candidate_str: &str,
) -> Result<Option<Value>, HintError> {
    let extension = candidate_str.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("json") => {
            let Some(body) = read_text(project_dir, candidate)? else {
                return Ok(None);
            };
            Ok(serde_json::from_str(&body).ok())
        }
        Some("yaml" | "yml") => {
            let Some(body) = read_text(project_dir, candidate)? else {
                return Ok(None);
            };
            Ok(serde_saphyr::from_str(&body).ok())
        }
        Some("md") => Ok(model
            .frontmatter
            .iter()
            .find(|fm| fm.path == candidate_str)
            .map(|fm| Value::Object(fm.fields.clone()))),
        _ => Ok(None),
    }
}

fn read_text(project_dir: &Path, candidate: &Path) -> Result<Option<String>, HintError> {
    let absolute = project_dir.join(candidate);
    match std::fs::read_to_string(&absolute) {
        Ok(body) => Ok(Some(body)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(HintError::Filesystem {
            op: "read",
            path: absolute,
            source: err,
        }),
    }
}
