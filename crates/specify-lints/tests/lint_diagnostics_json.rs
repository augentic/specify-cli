//! `Format::Json` formatter — `LintResult` envelope wire envelope.
//!
//! Locks two invariants S9 (`specrun lint --format json`) and any
//! future Option-A consumer (`specdev lint --format json`) depend on:
//!
//! 1. The valid envelope round-trips: `render` schema-validates and
//!    the pretty-printed body deserialises back to an equal
//!    [`LintResult`].
//! 2. A handcrafted bad envelope (`version: 2`) is rejected with
//!    [`RenderError::JsonSchemaValidation`] before any bytes leave
//!    the formatter.

mod common;

use jsonschema::{Registry, Resource};
use serde_json::{Value, json};
use specify_diagnostics::render::json as json_formatter;
use specify_lints::lint::diagnostics::{Format, LintResult, RenderError, render};
use specify_schema::{LINT_FINDING_JSON_SCHEMA, LINT_RESULT_JSON_SCHEMA};

use crate::common::make_fixture;

const FINDING_SCHEMA_URL: &str =
    "https://github.com/augentic/specify-cli/schemas/lint/finding.schema.json";

fn envelope_validator() -> jsonschema::Validator {
    let envelope: Value =
        serde_json::from_str(LINT_RESULT_JSON_SCHEMA).expect("lint-result schema parses");
    let finding: Value =
        serde_json::from_str(LINT_FINDING_JSON_SCHEMA).expect("finding schema parses");
    let registry = Registry::new()
        .add(FINDING_SCHEMA_URL, Resource::from_contents(finding))
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry prepares");
    jsonschema::options()
        .with_registry(&registry)
        .build(&envelope)
        .expect("envelope schema compiles")
}

#[test]
fn round_trips_through_schema() {
    let fixture = make_fixture();
    let rendered = render(Format::Json, &fixture).expect("json render succeeds");
    assert!(rendered.ends_with('\n'), "render must terminate with a newline");

    let value: Value = serde_json::from_str(&rendered).expect("rendered text parses as JSON");
    let validator = envelope_validator();
    let errors: Vec<String> = validator.iter_errors(&value).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "rendered envelope must schema-validate; errors: {errors:?}");

    let parsed: LintResult = serde_json::from_value(value).expect("rendered envelope deserialises");
    assert_eq!(parsed, fixture);
}

#[test]
fn json_formatter_rejects_bad_envelope() {
    let bad = json!({
        "version": 2,
        "summary": { "critical": 0, "important": 0, "suggestion": 0, "optional": 0 },
        "findings": []
    });
    let err = json_formatter::render_value(&bad).expect_err("bad envelope must be rejected");
    let detail = match err {
        RenderError::JsonSchemaValidation { detail } => detail,
        other => panic!("expected JsonSchemaValidation, got {other:?}"),
    };
    assert!(!detail.is_empty(), "validation error must carry a detail string");
}

#[test]
fn rejects_missing_summary_key() {
    let bad = json!({
        "version": 1,
        "summary": { "critical": 0, "important": 0, "suggestion": 0 },
        "findings": []
    });
    let err = json_formatter::render_value(&bad).expect_err("missing summary key must be rejected");
    assert!(matches!(err, RenderError::JsonSchemaValidation { .. }));
}
