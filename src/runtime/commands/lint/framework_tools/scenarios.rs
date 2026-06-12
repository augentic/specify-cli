//! In-process `scenarios` framework checker (Road B `kind: tool`).
//!
//! Covers the filesystem-only scenario family: CORE-028 (artifact-path
//! safety), CORE-029 (body↔frontmatter id), CORE-031 (recorded-trace
//! header validation), CORE-033 (stage contiguity), and CORE-056
//! (catalog↔runs drift, policy via the rule's forwarded `config:`).
//! CORE-030 and CORE-032 are Road A declarative hints over the
//! `scenario` fact family.

mod catalog;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value as JsonValue;

use super::support::{ToolFinding, parsed_config, relative_display, requested_rule, walk_files};

const RULE_ARTIFACT_PATH_UNSAFE: &str = "CORE-028";
const RULE_BODY_ID_MISMATCH: &str = "CORE-029";
const RULE_RECORDED_TRACE_VIOLATION: &str = "CORE-031";
const RULE_STAGES_NOT_CONTIGUOUS: &str = "CORE-033";

const RULES: &[&str] = &[
    RULE_ARTIFACT_PATH_UNSAFE,
    RULE_BODY_ID_MISMATCH,
    catalog::RULE_CATALOG_RUNS_DRIFT,
    RULE_RECORDED_TRACE_VIOLATION,
    RULE_STAGES_NOT_CONTIGUOUS,
];

/// The fixed slice-loop stage order a scenario's `stages` list must be a
/// contiguous slice of, anchored at any element.
const STAGES_ORDER: [&str; 5] = ["plan", "refine", "build", "merge", "drop"];

/// Required fields on a `recorded-trace-header` first line (CORE-031).
const TRACE_REQUIRED_FIELDS: [&str; 6] =
    ["kind", "schemaVersion", "sourceBackend", "sourceRunId", "sourceTimestamp", "scenarioId"];

/// Run the scenario family scoped by the candidate sentinel path.
pub fn run(project_dir: &Path, args: &[String]) -> Vec<ToolFinding> {
    let scoped = requested_rule(args, RULES);
    let config = parsed_config(args);
    let mut findings: Vec<ScenarioFinding> = run_with_config(project_dir, config.as_ref())
        .into_iter()
        .filter(|finding| scoped.is_none_or(|rule| finding.rule_id == rule))
        .collect();
    findings
        .sort_by(|a, b| (a.rule_id, &a.path, &a.message).cmp(&(b.rule_id, &b.path, &b.message)));
    findings.into_iter().map(wire_finding).collect()
}

fn wire_finding(finding: ScenarioFinding) -> ToolFinding {
    let (impact, remediation) = guidance(finding.rule_id);
    ToolFinding {
        rule_id: finding.rule_id,
        path: finding.path,
        message: finding.message,
        impact,
        remediation,
    }
}

/// Per-rule operator-facing impact / remediation prose.
fn guidance(rule_id: &str) -> (&'static str, &'static str) {
    match rule_id {
        RULE_ARTIFACT_PATH_UNSAFE => (
            "A scenario declares an expected artifact path that is empty, absolute, or escapes the scenario workspace.",
            "Rewrite each `expected-artifacts` entry as a non-empty path relative to the scenario workspace, with no leading '/' or '..' segments.",
        ),
        RULE_BODY_ID_MISMATCH => (
            "A scenario's visible 'Scenario ID' body line disagrees with its frontmatter id, so readers cannot trust the citation.",
            "Align the body 'Scenario ID: `…`' line with the frontmatter `id`.",
        ),
        RULE_RECORDED_TRACE_VIOLATION => (
            "A recorded-trace file's first line is not a well-formed `recorded-trace-header`, so replay cannot trust its provenance.",
            "Make the first line a JSON `recorded-trace-header` object with schemaVersion 1 and every required field populated.",
        ),
        catalog::RULE_CATALOG_RUNS_DRIFT => (
            "The scenario catalog, the scenario files, and the committed run records disagree, so the catalog's gate status cannot be trusted.",
            "Reconcile the catalog row with the scenario tree and evals/runs/: status-bearing rows need exactly one committed record whose <result> agrees.",
        ),
        _ => (
            "Scenario stages are not a contiguous slice of the slice loop; the pack does not describe a runnable lifecycle window.",
            "Reorder the scenario's `stages` list to a contiguous run of [plan, refine, build, merge, drop] anchored at any element.",
        ),
    }
}

/// One scenario-pack violation before wire guidance is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScenarioFinding {
    rule_id: &'static str,
    path: Option<String>,
    message: String,
}

/// Run every scenario-pack check rooted at `project_dir`, including the
/// config-driven catalog↔runs check (CORE-056) when the forwarded rule
/// `config:` carries a catalog policy.
fn run_with_config(project_dir: &Path, config: Option<&JsonValue>) -> Vec<ScenarioFinding> {
    let mut findings = validate_scenario_frontmatter(project_dir);
    findings.extend(check_recorded_trace_freshness(project_dir));
    findings.extend(catalog::findings_from_config(project_dir, config));
    findings
}

/// An opted-in scenario file with its parsed frontmatter and body.
struct ScenarioFile {
    rel: String,
    content: String,
    frontmatter: BTreeMap<String, JsonValue>,
}

fn validate_scenario_frontmatter(project_dir: &Path) -> Vec<ScenarioFinding> {
    let opted = collect_opted_scenarios(project_dir);
    let mut findings = check_stages(&opted);
    findings.extend(check_body_id(&opted));
    findings.extend(check_artifact_paths(&opted));
    findings
}

/// Read and parse every discovered scenario, returning the opted-in
/// files. An opted-in file whose YAML fails to parse is skipped here;
/// the Road A `scenario` schema hint flags it instead (CORE-032).
fn collect_opted_scenarios(project_dir: &Path) -> Vec<ScenarioFile> {
    let mut opted = Vec::new();
    for path in discover_scenario_candidates(project_dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = relative_display(project_dir, &path);
        let Some(block) = frontmatter_block(&content) else {
            continue;
        };
        if let Ok(frontmatter) = serde_saphyr::from_str::<BTreeMap<String, JsonValue>>(block) {
            opted.push(ScenarioFile {
                rel,
                content,
                frontmatter,
            });
        }
    }
    opted
}

/// CORE-033: each non-empty frontmatter's `stages` must be a contiguous
/// slice-loop prefix.
fn check_stages(opted: &[ScenarioFile]) -> Vec<ScenarioFinding> {
    let mut findings = Vec::new();
    for sc in opted {
        if sc.frontmatter.is_empty() {
            continue;
        }
        if let Some(stages) = sc.frontmatter.get("stages")
            && !is_contiguous_stages_prefix(stages)
        {
            findings.push(ScenarioFinding {
                rule_id: RULE_STAGES_NOT_CONTIGUOUS,
                path: Some(sc.rel.clone()),
                message: format!(
                    "Scenario frontmatter: {} — stages must be a contiguous slice of \
                     [plan, refine, build, merge, drop] anchored at any element; got {}",
                    sc.rel,
                    serde_json::to_string(stages).unwrap_or_else(|_| "<?>".into())
                ),
            });
        }
    }
    findings
}

/// CORE-029: the body `Scenario ID:` line must match the frontmatter id.
fn check_body_id(opted: &[ScenarioFile]) -> Vec<ScenarioFinding> {
    let scenario_id_body_re =
        Regex::new(r"(?m)^Scenario ID:\s*`?([a-z][a-z0-9-]*)`?\s*$").expect("valid regex");
    let mut findings = Vec::new();
    for sc in opted {
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
            findings.push(ScenarioFinding {
                rule_id: RULE_BODY_ID_MISMATCH,
                path: Some(sc.rel.clone()),
                message: format!(
                    "Scenario frontmatter: {} — body 'Scenario ID: `{body_id}`' does not match \
                     frontmatter id '{id}'; align the visible line with the frontmatter id",
                    sc.rel
                ),
            });
        }
    }
    findings
}

/// CORE-028: every `expected-artifacts` entry must be a non-empty,
/// relative, non-escaping path.
fn check_artifact_paths(opted: &[ScenarioFile]) -> Vec<ScenarioFinding> {
    let mut findings = Vec::new();
    for sc in opted {
        let Some(JsonValue::Array(arts)) = sc.frontmatter.get("expected-artifacts") else {
            continue;
        };
        for art in arts {
            let Some(a) = art.as_str() else {
                continue;
            };
            let detail = if a.is_empty() {
                "expected-artifacts entry is empty".to_string()
            } else if a.starts_with('/') {
                format!(
                    "expected-artifact '{a}' must be relative to the scenario workspace, not \
                     absolute"
                )
            } else if a.split('/').any(|seg| seg == "..") {
                format!(
                    "expected-artifact '{a}' must not escape the scenario workspace ('..' segment \
                     not allowed)"
                )
            } else {
                continue;
            };
            findings.push(ScenarioFinding {
                rule_id: RULE_ARTIFACT_PATH_UNSAFE,
                path: Some(sc.rel.clone()),
                message: format!("Scenario frontmatter: {} — {detail}", sc.rel),
            });
        }
    }
    findings
}

/// CORE-031: recorded-trace header validation. The git-only staleness
/// advisory (CORE-034) is deliberately not covered.
fn check_recorded_trace_freshness(project_dir: &Path) -> Vec<ScenarioFinding> {
    let recorded_root = project_dir.join("evals").join("recorded");
    if !recorded_root.is_dir() {
        return Vec::new();
    }

    let mut trace_paths = Vec::new();
    walk_files(&recorded_root, &mut trace_paths);
    trace_paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"));
    trace_paths.sort();

    let mut findings = Vec::new();
    for path in &trace_paths {
        let rel = relative_display(project_dir, path);
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(source) => {
                findings.push(trace_finding(&rel, &format!("cannot read: {source}")));
                continue;
            }
        };

        let first_line = content.lines().next().unwrap_or("").trim();
        if first_line.is_empty() {
            findings.push(trace_finding(
                &rel,
                "empty file (expected a 'recorded-trace-header' line first)",
            ));
            continue;
        }

        let parsed: JsonValue = match serde_json::from_str(first_line) {
            Ok(value) => value,
            Err(source) => {
                findings
                    .push(trace_finding(&rel, &format!("first line is not valid JSON: {source}")));
                continue;
            }
        };

        if !parsed.is_object() {
            findings.push(trace_finding(&rel, "first line must be a JSON object"));
            continue;
        }

        let header = parsed;
        let kind = header.get("kind").and_then(JsonValue::as_str);
        if kind != Some("recorded-trace-header") {
            findings.push(trace_finding(
                &rel,
                &format!(
                    "first line kind must be 'recorded-trace-header' (got {})",
                    serde_json::to_string(header.get("kind").unwrap_or(&JsonValue::Null))
                        .unwrap_or_else(|_| "<unknown>".into())
                ),
            ));
            continue;
        }

        let schema_version = header.get("schemaVersion");
        if schema_version != Some(&JsonValue::Number(1.into())) {
            findings.push(trace_finding(
                &rel,
                &format!(
                    "recorded-trace-header.schemaVersion must be 1 (got {})",
                    serde_json::to_string(schema_version.unwrap_or(&JsonValue::Null))
                        .unwrap_or_else(|_| "<unknown>".into())
                ),
            ));
        }

        for field in TRACE_REQUIRED_FIELDS {
            let value = header.get(field);
            let missing = match value {
                None | Some(JsonValue::Null) => true,
                Some(JsonValue::String(s)) => s.is_empty(),
                _ => false,
            };
            if missing {
                findings.push(trace_finding(
                    &rel,
                    &format!("recorded-trace-header missing required field '{field}'"),
                ));
            }
        }
    }

    findings
}

fn trace_finding(rel: &str, detail: &str) -> ScenarioFinding {
    ScenarioFinding {
        rule_id: RULE_RECORDED_TRACE_VIOLATION,
        path: Some(rel.to_string()),
        message: format!("Recorded trace: {rel} — {detail}"),
    }
}

/// The `stages` array must be a contiguous run of [`STAGES_ORDER`]
/// anchored at any element.
fn is_contiguous_stages_prefix(stages: &JsonValue) -> bool {
    let Some(stages) = stages.as_array() else {
        return false;
    };
    if stages.is_empty() {
        return false;
    }
    let first = stages[0].as_str().unwrap_or("");
    let Some(start) = STAGES_ORDER.iter().position(|s| *s == first) else {
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

fn frontmatter_block(content: &str) -> Option<&str> {
    frontmatter_split(content).map(|(block, _)| block)
}

fn frontmatter_split(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let body_start = content.len() - (rest.len() - end) + "\n---".len();
    Some((&rest[..end], &content[body_start..]))
}

/// Discover scenario candidate files across the eval scenario pack,
/// target adapter tests, and plugin skill fixtures. Symlinks are never
/// traversed or collected.
fn discover_scenario_candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_eval_scenarios(&root.join("evals").join("scenarios"), &mut out);
    collect_target_scenarios(&root.join("adapters").join("targets"), &mut out);
    collect_plugin_fixture_scenarios(&root.join("plugins"), &mut out);
    out.sort();
    out.dedup();
    out
}

/// Flat `evals/scenarios/<id>.md` files (depth 1), skipping the pack
/// `README.md` catalog.
fn collect_eval_scenarios(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let is_md =
            path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if name == "README.md" || !is_md {
            continue;
        }
        out.push(path);
    }
}

/// `adapters/targets/<adapter>/tests/<file>.md` and
/// `adapters/targets/<adapter>/tests/<dir>/scenario.md`.
fn collect_target_scenarios(targets_dir: &Path, out: &mut Vec<PathBuf>) {
    let mut files = Vec::new();
    walk_files(targets_dir, &mut files);
    for path in files {
        let Ok(rel) = path.strip_prefix(targets_dir) else {
            continue;
        };
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        let ext_md = path.extension().and_then(|e| e.to_str()) == Some("md");
        if ext_md && parts.len() == 3 && parts[1] == "tests" {
            out.push(path.clone());
        }
        if parts.len() == 4 && parts[1] == "tests" && parts[3] == "scenario.md" {
            out.push(path);
        }
    }
}

/// `plugins/<plugin>/skills/<skill>/fixtures/<case>/scenario.md`.
fn collect_plugin_fixture_scenarios(plugins_dir: &Path, out: &mut Vec<PathBuf>) {
    let mut files = Vec::new();
    walk_files(plugins_dir, &mut files);
    for path in files {
        if path.file_name().and_then(|n| n.to_str()) != Some("scenario.md") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(plugins_dir) else {
            continue;
        };
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        if parts.len() == 6
            && parts[1] == "skills"
            && parts[3] == "fixtures"
            && parts[5] == "scenario.md"
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run_all(project_dir: &Path) -> Vec<ScenarioFinding> {
        run_with_config(project_dir, None)
    }

    #[test]
    fn contiguous_accepts_anchored_slice() {
        assert!(is_contiguous_stages_prefix(&json!(["refine", "build"])));
        assert!(is_contiguous_stages_prefix(&json!(["plan", "refine", "build", "merge", "drop"])));
    }

    #[test]
    fn contiguous_rejects_gap_and_unknown() {
        assert!(!is_contiguous_stages_prefix(&json!(["plan", "build"])));
        assert!(!is_contiguous_stages_prefix(&json!(["draft"])));
        assert!(!is_contiguous_stages_prefix(&json!([])));
    }

    fn write_scenario(dir: &Path, name: &str, body: &str) {
        let scenarios = dir.join("evals/scenarios");
        std::fs::create_dir_all(&scenarios).expect("mkdir");
        std::fs::write(scenarios.join(name), body).expect("write scenario");
    }

    /// A fully schema-valid scenario frontmatter block keyed by `id`.
    fn valid_frontmatter(id: &str) -> String {
        format!(
            "---\nid: {id}\nowner: spec\nkind: skill\nentrypoint: /spec:refine\nstages: [refine, build]\nisolation: fresh-project\n---\n\nBody.\n"
        )
    }

    fn flagged(findings: &[ScenarioFinding], rule_id: &str) -> Vec<Option<String>> {
        findings.iter().filter(|f| f.rule_id == rule_id).map(|f| f.path.clone()).collect()
    }

    #[test]
    fn flags_non_contiguous_eval_scenario() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_scenario(dir.path(), "good.md", &valid_frontmatter("good"));
        let mut bad = valid_frontmatter("bad");
        bad = bad.replace("[refine, build]", "[plan, build]");
        write_scenario(dir.path(), "bad.md", &bad);

        let stages = flagged(&run_all(dir.path()), RULE_STAGES_NOT_CONTIGUOUS);
        assert_eq!(stages, vec![Some("evals/scenarios/bad.md".to_string())]);
    }

    #[test]
    fn flags_unsafe_expected_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut body = valid_frontmatter("arts");
        body = body.replace(
            "isolation: fresh-project\n",
            "isolation: fresh-project\nexpected-artifacts: ['../escape.txt']\n",
        );
        write_scenario(dir.path(), "arts.md", &body);
        let unsafe_paths = flagged(&run_all(dir.path()), RULE_ARTIFACT_PATH_UNSAFE);
        assert_eq!(unsafe_paths, vec![Some("evals/scenarios/arts.md".to_string())]);
    }

    #[test]
    fn flags_body_id_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!("{}\nScenario ID: `other`\n", valid_frontmatter("real"));
        write_scenario(dir.path(), "mismatch.md", &body);
        let mismatch = flagged(&run_all(dir.path()), RULE_BODY_ID_MISMATCH);
        assert_eq!(mismatch, vec![Some("evals/scenarios/mismatch.md".to_string())]);
    }

    #[test]
    fn flags_recorded_trace_violation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recorded = dir.path().join("evals/recorded/run-1");
        std::fs::create_dir_all(&recorded).expect("mkdir");
        std::fs::write(recorded.join("trace.jsonl"), "not json\n").expect("write trace");
        let trace = flagged(&run_all(dir.path()), RULE_RECORDED_TRACE_VIOLATION);
        assert_eq!(trace, vec![Some("evals/recorded/run-1/trace.jsonl".to_string())]);
    }

    #[test]
    fn clean_tree_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_scenario(dir.path(), "ok.md", &valid_frontmatter("ok"));
        assert!(run_all(dir.path()).is_empty(), "a schema-valid contiguous scenario flags nothing");
    }
}
