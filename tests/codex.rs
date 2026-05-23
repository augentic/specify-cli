//! Schema tests for codex rule frontmatter.
//!
//! These tests intentionally validate only the YAML frontmatter contract.

use std::fs;
use std::path::PathBuf;

use jsonschema::Validator;
use serde_json::Value as JsonValue;

mod common;
use common::repo_root;

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

#[test]
fn schema_validates_minimal_frontmatter() {
    assert_valid_fixture("codex-valid-minimal.md");
}

#[test]
fn schema_accepts_required_body_heading() {
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
fn schema_rejects_missing_required_fields() {
    let validator = load_validator();
    let instance = frontmatter_fixture("codex-invalid-missing-title.md");
    assert!(
        !validator.is_valid(&instance),
        "missing required title should fail frontmatter validation"
    );
}

#[test]
fn schema_rejects_unknown_severity() {
    assert_invalid_fixture_at("codex-invalid-unknown-severity.md", "/severity");
}

#[test]
fn schema_rejects_malformed_rule_id() {
    assert_invalid_fixture_at("codex-invalid-malformed-id.md", "/id");
}
