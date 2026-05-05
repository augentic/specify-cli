//! Integration tests for `schemas/plan/plan.schema.json` plus the
//! kebab-name regex shared with `specify_slice::actions::validate_name`.
//!
//! The schema tests are pure-library: they compile the bundled JSON
//! Schema and feed it YAML fixtures converted to `serde_json::Value`.
//! CLI integration tests for the `specify plan *` group that
//! used to live alongside these schema tests have moved to
//! `tests/initiative.rs` (see RFC-2 §3 — CLI namespace rename).

use std::fs;
use std::path::PathBuf;

use jsonschema::Validator;
use serde_json::Value as JsonValue;

/// RFC-2 §"The Plan" `platform-v2` example, inline.
///
/// Kept inline (rather than loaded from a fixture) so the test is pinned to
/// the exact shape the RFC ships; subsequent Changes that touch the RFC must
/// also touch this constant.
const RFC_EXAMPLE: &str = r"
name: platform-v2

sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git

changes:
  - name: user-registration
    sources: [monolith]
    status: done

  - name: email-verification
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress

  - name: registration-duplicate-email-crash
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending

  - name: notification-preferences
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending

  - name: extract-shared-validation
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending

  - name: product-catalog
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending

  - name: shopping-cart
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending

  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.

  - name: checkout-ui
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
";

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/plan/plan.schema.json")
}

fn load_validator() -> Validator {
    let raw = fs::read_to_string(schema_path()).expect("read plan.schema.json");
    let schema: JsonValue = serde_json::from_str(&raw).expect("plan.schema.json is valid JSON");
    jsonschema::validator_for(&schema).expect("plan.schema.json compiles as a JSON Schema")
}

fn yaml_to_json(yaml: &str) -> JsonValue {
    serde_saphyr::from_str(yaml).expect("fixture parses as YAML")
}

#[test]
fn plan_schema_validates_rfc_example() {
    let validator = load_validator();
    let instance = yaml_to_json(RFC_EXAMPLE);
    let errors: Vec<String> =
        validator.iter_errors(&instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();
    assert!(errors.is_empty(), "RFC-2 example should validate cleanly; errors: {errors:#?}");
}

#[test]
fn plan_schema_rejects_unknown_status_value() {
    let validator = load_validator();
    let mutated = RFC_EXAMPLE.replacen("status: in-progress", "status: maybe", 1);
    let instance = yaml_to_json(&mutated);

    let offending_paths: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| e.instance_path().to_string())
        .filter(|p| p.starts_with("/changes/") && p.ends_with("/status"))
        .collect();

    assert!(
        !offending_paths.is_empty(),
        "unknown status should produce at least one error on /changes/*/status; got none"
    );
}

#[test]
fn plan_schema_rejects_non_kebab_name() {
    let validator = load_validator();
    let mutated = RFC_EXAMPLE.replacen("name: platform-v2", "name: Platform V2", 1);
    let instance = yaml_to_json(&mutated);

    let name_errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| e.instance_path().to_string())
        .filter(|p| p == "/name")
        .collect();

    assert!(
        !name_errors.is_empty(),
        "non-kebab-case name should produce at least one error on /name; got none"
    );
}

/// The JSON Schema regex and `validate_name` must agree on every name,
/// in both directions. The cases below are the ones RFC-2 §1.5 calls
/// out; keep them in sync with the doc-comment on
/// `specify_slice::actions::validate_name`.
#[test]
fn kebab_name_regex_matches_validate_name() {
    use regex::Regex;
    use specify::slice_actions;

    // Extract the pattern from the compiled schema to keep this test
    // honest against drift — the schema file is the source of truth.
    let raw = fs::read_to_string(schema_path()).expect("read plan.schema.json");
    let schema: JsonValue = serde_json::from_str(&raw).expect("plan.schema.json is valid JSON");
    let pattern = schema["$defs"]["kebabName"]["pattern"]
        .as_str()
        .expect("$defs.kebabName.pattern is a string");
    let regex = Regex::new(pattern).expect("$defs.kebabName.pattern compiles as a regex");

    let accept = ["a", "ab", "a-b", "a1", "user-registration"];
    let reject = ["", "-a", "a-", "a--b", "A", "a_b"];

    for name in accept {
        assert!(regex.is_match(name), "regex `{pattern}` should accept `{name}` but did not");
        slice_actions::validate_name(name).unwrap_or_else(|err| {
            panic!("validate_name should accept `{name}`, got error: {err}");
        });
    }

    for name in reject {
        assert!(!regex.is_match(name), "regex `{pattern}` should reject `{name}` but accepted it");
        slice_actions::validate_name(name)
            .err()
            .unwrap_or_else(|| panic!("validate_name should reject `{name}`"));
    }
}
