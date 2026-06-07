//! Pure scenario-pack checks for the `scenarios` framework-authoring
//! tool, lifted from the host CLI's retiring `ScenariosCheck` imperative
//! predicate (Road B framework tool).
//!
//! The tool covers the filesystem-only scenario family: CORE-028
//! (artifact-path safety), CORE-029 (body↔frontmatter id), CORE-030
//! (whole-tree duplicate id), CORE-031 (recorded-trace header
//! validation), CORE-032 (frontmatter schema), and CORE-033 (stage
//! contiguity). The discovery walk mirrors the host's
//! `discover_scenario_candidates`; every check mirrors its counterpart
//! in `framework::check::scenarios`. CORE-034's git-only staleness
//! advisory is *not* lifted (it shells out to `git`, unfit for the WASI
//! sandbox, and is removed in Phase 8). Carve-out posture: this crate
//! owns its logic and embeds its own copy of `scenario.schema.json`,
//! depending only on `serde` / `serde-saphyr` / `serde_json` /
//! `jsonschema` / `regex`, never the host diagnostics crate (`main.rs`
//! renders the wire envelope).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value as JsonValue;

/// Codex ids each check stamps onto its findings (closed `CORE-NNN`).
pub const RULE_ARTIFACT_PATH_UNSAFE: &str = "CORE-028";
pub const RULE_BODY_ID_MISMATCH: &str = "CORE-029";
pub const RULE_DUPLICATE_ID: &str = "CORE-030";
pub const RULE_RECORDED_TRACE_VIOLATION: &str = "CORE-031";
pub const RULE_SCHEMA_VIOLATION: &str = "CORE-032";
pub const RULE_STAGES_NOT_CONTIGUOUS: &str = "CORE-033";

/// Tool-owned copy of the canonical scenario frontmatter schema
/// (`schemas/authoring/scenario.schema.json`). Embedded so the tool
/// never reaches back into the host engine for policy (Road B B-2).
const SCENARIO_SCHEMA_SOURCE: &str = include_str!("../embedded/scenario.schema.json");

/// The fixed slice-loop stage order a scenario's `stages` list must be a
/// contiguous slice of, anchored at any element.
const STAGES_ORDER: [&str; 5] = ["plan", "refine", "build", "merge", "drop"];

/// Required fields on a `recorded-trace-header` first line (CORE-031).
const TRACE_REQUIRED_FIELDS: [&str; 6] =
    ["kind", "schemaVersion", "sourceBackend", "sourceRunId", "sourceTimestamp", "scenarioId"];

/// One scenario-pack violation: its codex `rule_id`, an optional
/// project-relative path, and a human-readable message. The caller
/// stamps the wire severity (always `important` for this family).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file, or
    /// `None` for whole-tree findings (duplicate id, schema infra).
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// Run every scenario-pack check rooted at `project_dir` (the framework
/// tree) and return all findings across the CORE-028..033 family,
/// sorted by `(rule_id, path, message)` for a stable wire order. The
/// caller scopes the set to a single rule before emitting.
#[must_use]
pub fn run(project_dir: &Path) -> Vec<ScenarioFinding> {
    let mut findings = validate_scenario_frontmatter(project_dir);
    findings.extend(check_recorded_trace_freshness(project_dir));
    findings.sort_by(|a, b| {
        (a.rule_id, &a.path, &a.message).cmp(&(b.rule_id, &b.path, &b.message))
    });
    findings
}

/// An opted-in scenario file with its parsed frontmatter and body.
struct ScenarioFile {
    rel: String,
    content: String,
    frontmatter: BTreeMap<String, JsonValue>,
}

/// Lazily compiled scenario validator built from the embedded schema.
fn scenario_validator() -> Result<&'static jsonschema::Validator, String> {
    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
    VALIDATOR
        .get_or_init(|| {
            let schema: JsonValue = serde_json::from_str(SCENARIO_SCHEMA_SOURCE)
                .map_err(|err| format!("embedded scenario.schema.json is not JSON: {err}"))?;
            jsonschema::validator_for(&schema)
                .map_err(|err| format!("embedded scenario.schema.json failed to compile: {err}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Run the frontmatter family of checks: schema (CORE-032), stages
/// (CORE-033), body-id (CORE-029), artifact-path (CORE-028), and
/// whole-tree duplicate id (CORE-030). Mirrors the host's
/// `validate_scenario_frontmatter`.
fn validate_scenario_frontmatter(project_dir: &Path) -> Vec<ScenarioFinding> {
    let validator = match scenario_validator() {
        Ok(validator) => validator,
        Err(error) => {
            return vec![ScenarioFinding {
                rule_id: RULE_SCHEMA_VIOLATION,
                path: None,
                message: format!("Scenario frontmatter: cannot load scenario schema: {error}"),
            }];
        }
    };

    let (opted, mut findings) = collect_opted_scenarios(project_dir);
    findings.extend(check_schema(validator, &opted));
    findings.extend(check_stages(&opted));
    findings.extend(check_body_id(&opted));
    findings.extend(check_artifact_paths(&opted));
    findings.extend(check_duplicate_ids(&opted));
    findings
}

/// Read and parse every discovered scenario, returning the opted-in
/// files plus any YAML-parse `scenarios.schema-violation` findings.
fn collect_opted_scenarios(project_dir: &Path) -> (Vec<ScenarioFile>, Vec<ScenarioFinding>) {
    let mut opted = Vec::new();
    let mut findings = Vec::new();
    for path in discover_scenario_candidates(project_dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = relative_display(project_dir, &path);
        let Some(block) = frontmatter_block(&content) else {
            continue;
        };
        match parse_frontmatter_yaml(block) {
            Ok(frontmatter) => opted.push(ScenarioFile {
                rel,
                content,
                frontmatter,
            }),
            Err(msg) => findings.push(ScenarioFinding {
                rule_id: RULE_SCHEMA_VIOLATION,
                path: Some(rel.clone()),
                message: format!("Scenario frontmatter: {rel} — invalid YAML: {msg}"),
            }),
        }
    }
    (opted, findings)
}

/// CORE-032: validate each opted file's frontmatter against the embedded
/// scenario schema, one finding per schema error.
fn check_schema(validator: &jsonschema::Validator, opted: &[ScenarioFile]) -> Vec<ScenarioFinding> {
    let mut findings = Vec::new();
    for sc in opted {
        let value = JsonValue::Object(sc.frontmatter.clone().into_iter().collect());
        for error in validator.iter_errors(&value) {
            let instance_path = error.instance_path().to_string();
            let at = if instance_path.is_empty() { "/".to_string() } else { instance_path };
            findings.push(ScenarioFinding {
                rule_id: RULE_SCHEMA_VIOLATION,
                path: Some(sc.rel.clone()),
                message: format!("Scenario frontmatter: {} — {} {}", sc.rel, at, error)
                    .trim()
                    .to_string(),
            });
        }
    }
    findings
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

/// CORE-030: scenario ids must be unique across the whole tree.
fn check_duplicate_ids(opted: &[ScenarioFile]) -> Vec<ScenarioFinding> {
    let mut ids_by_value: HashMap<String, Vec<String>> = HashMap::new();
    for sc in opted {
        let Some(JsonValue::String(id)) = sc.frontmatter.get("id") else {
            continue;
        };
        ids_by_value.entry(id.clone()).or_default().push(sc.rel.clone());
    }
    let mut findings = Vec::new();
    for (id, mut paths) in ids_by_value {
        if paths.len() > 1 {
            paths.sort();
            findings.push(ScenarioFinding {
                rule_id: RULE_DUPLICATE_ID,
                path: None,
                message: format!(
                    "Scenario frontmatter: duplicate scenario id '{id}' across files: {}",
                    paths.join(", ")
                ),
            });
        }
    }
    findings
}

/// Run recorded-trace header validation (CORE-031). Mirrors the
/// filesystem half of the host's `check_recorded_trace_freshness`; the
/// git-only staleness advisory (CORE-034) is deliberately not lifted.
fn check_recorded_trace_freshness(project_dir: &Path) -> Vec<ScenarioFinding> {
    let recorded_root = project_dir.join("acceptance").join("recorded");
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
                findings.push(trace_finding(&rel, &format!("first line is not valid JSON: {source}")));
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

/// Port of the host's `is_contiguous_stages_prefix`: the `stages` array
/// must be a contiguous run of [`STAGES_ORDER`] anchored at any element.
#[must_use]
pub fn is_contiguous_stages_prefix(stages: &JsonValue) -> bool {
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

fn parse_frontmatter_yaml(block: &str) -> Result<BTreeMap<String, JsonValue>, String> {
    serde_saphyr::from_str(block).map_err(|err| err.to_string())
}

/// Extract the YAML block between a leading `---` line and its closing
/// `---`. Mirrors the host frontmatter splitter.
fn frontmatter_block(content: &str) -> Option<&str> {
    frontmatter_split(content).map(|(block, _)| block)
}

/// Split into the frontmatter block and the body following the closing
/// `---` delimiter.
fn frontmatter_split(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let body_start = content.len() - (rest.len() - end) + "\n---".len();
    Some((&rest[..end], &content[body_start..]))
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Discover scenario candidate files across the acceptance scenario pack,
/// target adapter tests, and plugin skill fixtures — mirroring the host's
/// `discover_scenario_candidates`. Symlinks are never traversed or
/// collected (the host walks with `follow_links(false)` and skips
/// symlinked plugin fixtures).
fn discover_scenario_candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_acceptance_scenarios(&root.join("acceptance").join("scenarios"), &mut out);
    collect_target_scenarios(&root.join("adapters").join("targets"), &mut out);
    collect_plugin_fixture_scenarios(&root.join("plugins"), &mut out);
    out.sort();
    out.dedup();
    out
}

/// Flat `acceptance/scenarios/<id>.md` files (depth 1), skipping the pack
/// `README.md` catalog.
fn collect_acceptance_scenarios(dir: &Path, out: &mut Vec<PathBuf>) {
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

/// Recursive file collector that never follows or records symlinks (so a
/// symlinked directory is not traversed and a symlinked file is not
/// collected), matching the host's `follow_links(false)` + symlink-skip
/// discovery posture.
fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            walk_files(&path, out);
        } else if file_type.is_file() {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
        let scenarios = dir.join("acceptance/scenarios");
        std::fs::create_dir_all(&scenarios).expect("mkdir");
        std::fs::write(scenarios.join(name), body).expect("write scenario");
    }

    /// A fully schema-valid scenario frontmatter block keyed by `id`.
    fn valid_frontmatter(id: &str) -> String {
        format!(
            "---\nid: {id}\nowner: spec\nkind: skill\nbackend: manual\nentrypoint: /spec:refine\nstages: [refine, build]\nisolation: fresh-project\n---\n\nBody.\n"
        )
    }

    fn flagged(findings: &[ScenarioFinding], rule_id: &str) -> Vec<Option<String>> {
        findings.iter().filter(|f| f.rule_id == rule_id).map(|f| f.path.clone()).collect()
    }

    #[test]
    fn flags_non_contiguous_acceptance_scenario() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_scenario(
            dir.path(),
            "good.md",
            &valid_frontmatter("good"),
        );
        let mut bad = valid_frontmatter("bad");
        bad = bad.replace("[refine, build]", "[plan, build]");
        write_scenario(dir.path(), "bad.md", &bad);

        let stages = flagged(&run(dir.path()), RULE_STAGES_NOT_CONTIGUOUS);
        assert_eq!(stages, vec![Some("acceptance/scenarios/bad.md".to_string())]);
    }

    #[test]
    fn flags_schema_violation_for_missing_required_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_scenario(dir.path(), "thin.md", "---\nid: thin\nstages: [refine, build]\n---\n\nBody.\n");
        let schema = flagged(&run(dir.path()), RULE_SCHEMA_VIOLATION);
        assert!(!schema.is_empty(), "missing required fields must flag CORE-032");
        assert!(schema.iter().all(|p| p.as_deref() == Some("acceptance/scenarios/thin.md")));
    }

    #[test]
    fn flags_duplicate_id_across_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_scenario(dir.path(), "a.md", &valid_frontmatter("shared"));
        write_scenario(dir.path(), "b.md", &valid_frontmatter("shared"));
        let dup = flagged(&run(dir.path()), RULE_DUPLICATE_ID);
        assert_eq!(dup, vec![None], "duplicate id is whole-tree (no single path)");
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
        let unsafe_paths = flagged(&run(dir.path()), RULE_ARTIFACT_PATH_UNSAFE);
        assert_eq!(unsafe_paths, vec![Some("acceptance/scenarios/arts.md".to_string())]);
    }

    #[test]
    fn flags_body_id_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!("{}\nScenario ID: `other`\n", valid_frontmatter("real"));
        write_scenario(dir.path(), "mismatch.md", &body);
        let mismatch = flagged(&run(dir.path()), RULE_BODY_ID_MISMATCH);
        assert_eq!(mismatch, vec![Some("acceptance/scenarios/mismatch.md".to_string())]);
    }

    #[test]
    fn flags_recorded_trace_violation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recorded = dir.path().join("acceptance/recorded/run-1");
        std::fs::create_dir_all(&recorded).expect("mkdir");
        std::fs::write(recorded.join("trace.jsonl"), "not json\n").expect("write trace");
        let trace = flagged(&run(dir.path()), RULE_RECORDED_TRACE_VIOLATION);
        assert_eq!(trace, vec![Some("acceptance/recorded/run-1/trace.jsonl".to_string())]);
    }

    #[test]
    fn clean_tree_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_scenario(dir.path(), "ok.md", &valid_frontmatter("ok"));
        assert!(run(dir.path()).is_empty(), "a schema-valid contiguous scenario flags nothing");
    }
}
