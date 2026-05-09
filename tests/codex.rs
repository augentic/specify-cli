//! Schema tests for codex rule frontmatter.
//!
//! These tests intentionally validate only the YAML frontmatter contract. The
//! parser will own Markdown body parsing, including the required `## Rule`
//! heading and duplicate-id validation across resolved rule sets.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use jsonschema::Validator;
use serde_json::Value as JsonValue;
use tempfile::{TempDir, tempdir};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn schema_path() -> PathBuf {
    repo_root().join("schemas/codex-rule.schema.json")
}

fn fixture_path(name: &str) -> PathBuf {
    repo_root().join("tests/fixtures").join(name)
}

fn load_validator() -> Validator {
    let raw = fs::read_to_string(schema_path()).expect("read codex-rule.schema.json");
    let schema: JsonValue =
        serde_json::from_str(&raw).expect("codex-rule.schema.json is valid JSON");
    jsonschema::validator_for(&schema).expect("codex-rule.schema.json compiles")
}

fn frontmatter_fixture(name: &str) -> JsonValue {
    let content = fs::read_to_string(fixture_path(name)).expect("read codex fixture");
    let mut parts = content.splitn(3, "---\n");
    assert_eq!(parts.next(), Some(""), "fixture must start with frontmatter");
    let yaml = parts.next().expect("fixture has closing frontmatter delimiter");
    serde_saphyr::from_str(yaml).expect("fixture frontmatter parses as YAML")
}

fn error_paths(validator: &Validator, instance: &JsonValue) -> Vec<String> {
    validator.iter_errors(instance).map(|e| e.instance_path().to_string()).collect()
}

fn assert_valid_fixture(name: &str) {
    let validator = load_validator();
    let instance = frontmatter_fixture(name);
    let errors = error_paths(&validator, &instance);
    assert!(errors.is_empty(), "{name} should validate cleanly; errors: {errors:#?}");
}

fn assert_invalid_fixture_at(name: &str, path: &str) {
    let validator = load_validator();
    let instance = frontmatter_fixture(name);
    let errors = error_paths(&validator, &instance);
    assert!(
        errors.iter().any(|candidate| candidate == path),
        "{name} should fail at {path}; got {errors:#?}"
    );
}

fn parse_json(stdout: &[u8]) -> JsonValue {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"))
}

struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
    fn new() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join(".specify")).expect("create .specify");
        fs::write(root.join(".specify/project.yaml"), "name: demo\ncapability: project\n")
            .expect("write project.yaml");
        write_capability(&root, "default", 1);
        write_capability(&root, "project", 2);
        Self { _tmp: tmp, root }
    }

    fn initialized_with_codex_distribution() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).expect("create project dir");

        let capabilities = tmp.path().join("capabilities");
        let default_root = write_capability_under(&capabilities, "default", 1);
        let project_root = write_capability_under(&capabilities, "project", 2);
        write_rule(&default_root, "001.md", "UNI-001");
        write_rule_with_body(
            &default_root,
            "002.md",
            "UNI-002",
            "critical",
            "External input crosses a boundary.",
            "\n## Rule\n\nValidate external input before trust.\n",
        );
        write_rule(&project_root, "project.md", "OMNIA-001");

        specify()
            .current_dir(&root)
            .args(["init"])
            .arg(&project_root)
            .args(["--name", "demo"])
            .assert()
            .success();
        assert!(
            root.join(".specify/.cache/default/capability.yaml").is_file(),
            "init should cache sibling default capability for codex resolution"
        );

        Self { _tmp: tmp, root }
    }

    fn root(&self) -> &std::path::Path {
        &self.root
    }
}

fn write_capability(project_dir: &std::path::Path, name: &str, version: u32) -> PathBuf {
    write_capability_under(&project_dir.join("schemas"), name, version)
}

fn write_capability_under(parent_dir: &std::path::Path, name: &str, version: u32) -> PathBuf {
    let root = parent_dir.join(name);
    fs::create_dir_all(&root).expect("create capability root");
    fs::write(
        root.join("capability.yaml"),
        format!(
            "\
name: {name}
version: {version}
description: {name} test capability
pipeline:
  define: []
  build: []
  merge: []
"
        ),
    )
    .expect("write capability manifest");
    root
}

fn write_rule(source_root: &std::path::Path, relative_path: &str, id: &str) -> PathBuf {
    write_rule_with_body(
        source_root,
        relative_path,
        id,
        "suggestion",
        "Test trigger.",
        "\n## Rule\n\nReview this behavior.\n",
    )
}

fn write_rule_with_body(
    source_root: &std::path::Path, relative_path: &str, id: &str, severity: &str, trigger: &str,
    body: &str,
) -> PathBuf {
    let path = source_root.join("codex").join(relative_path);
    fs::create_dir_all(path.parent().expect("rule path has parent")).expect("create codex dir");
    fs::write(
        &path,
        format!(
            "\
---
id: {id}
title: Test Rule {id}
severity: {severity}
trigger: {trigger}
---
{body}"
        ),
    )
    .expect("write codex rule");
    path
}

#[test]
fn codex_schema_validates_minimal_frontmatter() {
    assert_valid_fixture("codex-valid-minimal.md");
}

#[test]
fn codex_schema_accepts_only_required_body_heading_in_fixture() {
    let content =
        fs::read_to_string(fixture_path("codex-valid-minimal.md")).expect("read minimal fixture");
    assert!(content.contains("\n## Rule\n"), "fixture should include the required body heading");
    assert!(
        !content.contains("\n## Look For\n") && !content.contains("\n## Good\n"),
        "fixture should prove optional body sections are not part of the schema contract"
    );
    assert_valid_fixture("codex-valid-minimal.md");
}

#[test]
fn codex_schema_validates_optional_v1_metadata() {
    assert_valid_fixture("codex-valid-full.md");
}

#[test]
fn codex_schema_rejects_missing_required_fields() {
    let validator = load_validator();
    let instance = frontmatter_fixture("codex-invalid-missing-title.md");
    assert!(
        !validator.is_valid(&instance),
        "missing required title should fail frontmatter validation"
    );
}

#[test]
fn codex_schema_rejects_unknown_severity() {
    assert_invalid_fixture_at("codex-invalid-unknown-severity.md", "/severity");
}

#[test]
fn codex_schema_rejects_malformed_rule_id() {
    assert_invalid_fixture_at("codex-invalid-malformed-id.md", "/id");
}

#[test]
fn codex_schema_rejects_invalid_review_mode() {
    assert_invalid_fixture_at("codex-invalid-review-mode.md", "/review_mode");
}

#[test]
fn codex_help_lists_subcommands() {
    let assert = specify().args(["codex", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    for subcommand in ["list", "show", "validate", "export"] {
        assert!(
            stdout.contains(subcommand),
            "codex help should mention `{subcommand}`, got:\n{stdout}"
        );
    }
}

#[test]
fn codex_list_text_shows_id_severity_provenance_and_title() {
    let project = Project::new();
    let default_root = project.root().join("schemas/default");
    let project_root = project.root().join("schemas/project");
    write_rule(&default_root, "default.md", "UNI-001");
    write_rule(&project_root, "project.md", "OMNIA-001");

    let assert = specify().current_dir(project.root()).args(["codex", "list"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");

    assert!(stdout.contains("UNI-001"), "list should include default rule id:\n{stdout}");
    assert!(stdout.contains("suggestion"), "list should include severity:\n{stdout}");
    assert!(
        stdout.contains("capability default@v1"),
        "list should include default capability provenance:\n{stdout}"
    );
    assert!(stdout.contains("Test Rule OMNIA-001"), "list should include rule title:\n{stdout}");
}

#[test]
fn codex_show_text_prints_frontmatter_summary_and_body() {
    let project = Project::initialized_with_codex_distribution();

    let assert =
        specify().current_dir(project.root()).args(["codex", "show", "UNI-002"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");

    assert!(stdout.contains("id: UNI-002"), "show should include id:\n{stdout}");
    assert!(
        stdout.contains("severity: critical"),
        "show should include severity summary:\n{stdout}"
    );
    assert!(
        stdout.contains(".specify/.cache/default/codex/002.md"),
        "show should locate default rule through the init-populated cache:\n{stdout}"
    );
    assert!(stdout.contains("## Rule"), "show should print markdown body:\n{stdout}");
    assert!(stdout.contains("Validate external input"), "show should print rule prose:\n{stdout}");
}

#[test]
fn codex_export_json_includes_rules_body_paths_and_provenance() {
    let project = Project::new();
    let default_root = project.root().join("schemas/default");
    let project_root = project.root().join("schemas/project");
    write_rule(&default_root, "default.md", "UNI-001");
    write_rule(&project_root, "project.md", "OMNIA-001");

    let assert = specify()
        .current_dir(project.root())
        .args(["codex", "export", "--format", "json"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert!(value.get("schema-version").is_some(), "JSON envelope must include schema-version");
    assert_eq!(value["rule-count"], 2);
    let rules = value["rules"].as_array().expect("rules array");
    assert_eq!(rules[0]["id"], "UNI-001");
    assert_eq!(rules[0]["review-mode"], JsonValue::Null);
    assert_eq!(rules[0]["provenance-kind"], "capability");
    assert_eq!(rules[0]["capability-name"], "default");
    assert_eq!(rules[0]["capability-version"], 1);
    assert!(
        rules[0]["source-path"].as_str().unwrap().ends_with("schemas/default/codex/default.md")
    );
    assert!(rules[0]["body"].as_str().unwrap().contains("## Rule"));
}

#[test]
fn codex_export_json_resolves_initialized_cache_and_repo_overlay_in_order() {
    let project = Project::initialized_with_codex_distribution();
    write_rule(project.root(), "repo/overlay.md", "ORG-001");

    let assert = specify()
        .current_dir(project.root())
        .args(["codex", "export", "--format", "json"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["schema-version"], 3);
    assert_eq!(value["rule-count"], 4);
    let rules = value["rules"].as_array().expect("rules array");
    let ids: Vec<_> = rules.iter().map(|rule| rule["id"].as_str().expect("id str")).collect();
    assert_eq!(ids, ["UNI-001", "UNI-002", "OMNIA-001", "ORG-001"]);

    assert_eq!(rules[0]["provenance-kind"], "capability");
    assert_eq!(rules[0]["capability-name"], "default");
    assert_eq!(rules[0]["capability-version"], 1);
    assert!(
        rules[0]["source-path"].as_str().unwrap().ends_with(".specify/.cache/default/codex/001.md"),
        "default rule should come from the init-populated default cache: {}",
        rules[0]["source-path"]
    );
    assert_eq!(rules[0]["review-mode"], JsonValue::Null);

    assert_eq!(rules[2]["provenance-kind"], "capability");
    assert_eq!(rules[2]["capability-name"], "project");
    assert_eq!(rules[2]["capability-version"], 2);
    assert!(
        rules[2]["source-path"]
            .as_str()
            .unwrap()
            .ends_with(".specify/.cache/project/codex/project.md"),
        "project rule should come from the init-populated project cache: {}",
        rules[2]["source-path"]
    );

    assert_eq!(rules[3]["provenance-kind"], "repo");
    assert_eq!(rules[3]["capability-name"], JsonValue::Null);
    assert_eq!(rules[3]["capability-version"], JsonValue::Null);
    assert!(
        rules[3]["source-path"].as_str().unwrap().ends_with("codex/repo/overlay.md"),
        "repo overlay rule should come from repo-root codex: {}",
        rules[3]["source-path"]
    );
}

#[test]
fn codex_validate_clean_exits_zero() {
    let project = Project::new();
    let default_root = project.root().join("schemas/default");
    write_rule(&default_root, "default.md", "UNI-001");

    let assert =
        specify().current_dir(project.root()).args(["codex", "validate"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("Codex OK"), "validate should report pass summary:\n{stdout}");
}

#[test]
fn codex_validate_invalid_rule_exits_two() {
    let project = Project::new();
    let default_root = project.root().join("schemas/default");
    write_rule(&default_root, "default.md", "UNI-001");
    write_rule_with_body(
        project.root(),
        "broken.md",
        "ORG-001",
        "important",
        "A malformed rule exists.",
        "\n## Guidance\n\nMissing the required heading.\n",
    );

    let assert = specify().current_dir(project.root()).args(["codex", "validate"]).assert().code(2);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("Codex invalid"), "validate should report failure:\n{stdout}");
    assert!(
        stdout.contains("codex.body-has-rule-heading"),
        "validate should include failing validation rule id:\n{stdout}"
    );
    assert!(stdout.contains("broken.md"), "validate should include failing path:\n{stdout}");
}

#[test]
fn codex_validate_duplicate_ids_exits_two() {
    let project = Project::new();
    let default_root = project.root().join("schemas/default");
    let project_root = project.root().join("schemas/project");
    write_rule(&default_root, "default.md", "UNI-001");
    write_rule(&project_root, "project.md", "OMNIA-001");
    write_rule(project.root(), "repo.md", "OMNIA-001");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "codex", "validate"])
        .assert()
        .code(2);
    let value = parse_json(&assert.get_output().stdout);

    assert!(value.get("schema-version").is_some(), "JSON envelope must include schema-version");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error-count"], 1);
    assert_eq!(value["results"][0]["rule-id"], "codex.rule-id-unique");
    assert!(
        value["results"][0]["detail"].as_str().unwrap().contains("OMNIA-001"),
        "duplicate detail should name the id: {value}"
    );
}

#[test]
fn codex_show_missing_rule_id_fails() {
    let project = Project::new();
    let default_root = project.root().join("schemas/default");
    write_rule(&default_root, "default.md", "UNI-001");

    let assert = specify()
        .current_dir(project.root())
        .args(["codex", "show", "NOPE-999"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8 stderr");
    assert!(
        stderr.contains("codex-rule-not-found") && stderr.contains("NOPE-999"),
        "missing show should name the stable diagnostic and requested id:\n{stderr}"
    );
}
