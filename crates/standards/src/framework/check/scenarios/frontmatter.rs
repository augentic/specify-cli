//! Scenario frontmatter schema, stage-prefix, id, and artifact-path checks.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use regex::Regex;
use serde_json::Value as JsonValue;
use specify_diagnostics::Diagnostic;

use super::discovery::discover_scenario_candidates;
use super::{
    RULE_ARTIFACT_PATH_UNSAFE, RULE_BODY_ID_MISMATCH, RULE_DUPLICATE_ID, RULE_SCHEMA_VIOLATION,
    RULE_STAGES_NOT_CONTIGUOUS,
};
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::helpers::{frontmatter_block, frontmatter_split, relative_display};
use crate::framework::schema::{SchemaId, collect_errors};

const STAGES_ORDER: [&str; 5] = ["plan", "refine", "build", "merge", "drop"];

struct ScenarioFile {
    path: PathBuf,
    rel: String,
    content: String,
    frontmatter: BTreeMap<String, JsonValue>,
}

/// Run scenario frontmatter validation only (tests / direct invocation).
pub fn validate_scenario_frontmatter(ctx: &Context) -> Vec<Diagnostic> {
    let validator = match crate::framework::schema::validator(SchemaId::Scenario) {
        Ok(v) => v,
        Err(error) => {
            return vec![finding(
                RULE_SCHEMA_VIOLATION,
                format!("Scenario frontmatter: cannot load scenario schema: {error}"),
                None,
            )];
        }
    };

    let candidate_paths = discover_scenario_candidates(ctx);
    let mut opted = Vec::new();
    let mut findings = Vec::new();

    for path in candidate_paths {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let rel = relative_display(ctx.framework_root(), &path);
        let Some(fm_block) = frontmatter_block(&content) else {
            continue;
        };

        let fm = match parse_frontmatter_yaml(fm_block) {
            Ok(fm) => fm,
            Err(msg) => {
                findings.push(finding(
                    RULE_SCHEMA_VIOLATION,
                    format!("Scenario frontmatter: {rel} — invalid YAML: {msg}"),
                    Some(path),
                ));
                continue;
            }
        };

        opted.push(ScenarioFile {
            path,
            rel,
            content,
            frontmatter: fm,
        });
    }

    for sc in &opted {
        let value = JsonValue::Object(sc.frontmatter.clone().into_iter().collect());
        if let Err(errors) = collect_errors(&validator, &value) {
            for error in errors {
                let at = if error.instance_path.is_empty() {
                    "/".to_string()
                } else {
                    error.instance_path
                };
                findings.push(finding(
                    RULE_SCHEMA_VIOLATION,
                    format!("Scenario frontmatter: {} — {} {}", sc.rel, at, error.message)
                        .trim()
                        .to_string(),
                    Some(sc.path.clone()),
                ));
            }
        }
    }

    for sc in &opted {
        if sc.frontmatter.is_empty() {
            continue;
        }
        if let Some(stages) = sc.frontmatter.get("stages")
            && !is_contiguous_stages_prefix(stages)
        {
            findings.push(finding(
                RULE_STAGES_NOT_CONTIGUOUS,
                format!(
                    "Scenario frontmatter: {} — stages must be a contiguous slice of \
                     [plan, refine, build, merge, drop] anchored at any element; got {}",
                    sc.rel,
                    serde_json::to_string(stages).unwrap_or_else(|_| "<?>".into())
                ),
                Some(sc.path.clone()),
            ));
        }
    }

    let scenario_id_body_re =
        Regex::new(r"(?m)^Scenario ID:\s*`?([a-z][a-z0-9-]*)`?\s*$").expect("valid regex");

    for sc in &opted {
        let Some(JsonValue::String(id)) = sc.frontmatter.get("id") else {
            continue;
        };
        let Some((_, body)) = frontmatter_split(&sc.content) else {
            continue;
        };
        let Some(caps) = scenario_id_body_re.captures(body) else {
            continue;
        };
        let body_id = caps.get(1).expect("capture group").as_str();
        if body_id != id {
            findings.push(finding(
                RULE_BODY_ID_MISMATCH,
                format!(
                    "Scenario frontmatter: {} — body 'Scenario ID: `{body_id}`' does not match \
                     frontmatter id '{id}'; align the visible line with the frontmatter id",
                    sc.rel
                ),
                Some(sc.path.clone()),
            ));
        }
    }

    for sc in &opted {
        let Some(JsonValue::Array(arts)) = sc.frontmatter.get("expected-artifacts") else {
            continue;
        };
        for art in arts {
            let Some(a) = art.as_str() else {
                continue;
            };
            if a.is_empty() {
                findings.push(finding(
                    RULE_ARTIFACT_PATH_UNSAFE,
                    format!("Scenario frontmatter: {} — expected-artifacts entry is empty", sc.rel),
                    Some(sc.path.clone()),
                ));
                continue;
            }
            if a.starts_with('/') {
                findings.push(finding(
                    RULE_ARTIFACT_PATH_UNSAFE,
                    format!(
                        "Scenario frontmatter: {} — expected-artifact '{a}' must be relative to \
                         the scenario workspace, not absolute",
                        sc.rel
                    ),
                    Some(sc.path.clone()),
                ));
                continue;
            }
            if a.split('/').any(|seg| seg == "..") {
                findings.push(finding(
                    RULE_ARTIFACT_PATH_UNSAFE,
                    format!(
                        "Scenario frontmatter: {} — expected-artifact '{a}' must not escape the \
                         scenario workspace ('..' segment not allowed)",
                        sc.rel
                    ),
                    Some(sc.path.clone()),
                ));
            }
        }
    }

    let mut ids_by_value: HashMap<String, Vec<String>> = HashMap::new();
    for sc in &opted {
        let Some(JsonValue::String(id)) = sc.frontmatter.get("id") else {
            continue;
        };
        ids_by_value.entry(id.clone()).or_default().push(sc.rel.clone());
    }
    for (id, paths) in ids_by_value {
        if paths.len() > 1 {
            findings.push(finding(
                RULE_DUPLICATE_ID,
                format!(
                    "Scenario frontmatter: duplicate scenario id '{id}' across files: {}",
                    paths.join(", ")
                ),
                None,
            ));
        }
    }

    findings.sort_by(|a, b| a.title.cmp(&b.title));
    findings
}

fn is_contiguous_stages_prefix(stages: &JsonValue) -> bool {
    let Some(stages) = stages.as_array() else {
        return false;
    };
    if stages.is_empty() {
        return false;
    }
    let first = stages[0].as_str().unwrap_or("");
    let start = STAGES_ORDER.iter().position(|s| *s == first);
    let Some(start) = start else {
        return false;
    };
    for (i, stage) in stages.iter().enumerate() {
        if start + i >= STAGES_ORDER.len() {
            return false;
        }
        if stage.as_str() != Some(STAGES_ORDER[start + i]) {
            return false;
        }
    }
    true
}

fn parse_frontmatter_yaml(body: &str) -> Result<BTreeMap<String, JsonValue>, String> {
    serde_saphyr::from_str(body).map_err(|source| source.to_string())
}

#[cfg(test)]
mod unit_tests {
    use serde_json::json;

    use super::is_contiguous_stages_prefix;

    #[test]
    fn contiguous_stages_accepts_refine() {
        assert!(is_contiguous_stages_prefix(&json!(["refine", "build"])));
    }

    #[test]
    fn contiguous_stages_prefix_rejects_gap() {
        assert!(!is_contiguous_stages_prefix(&json!(["plan", "build"])));
    }

    #[test]
    fn contiguous_stages_rejects_unknown() {
        assert!(!is_contiguous_stages_prefix(&json!(["draft"])));
    }
}
