//! Smoke tests that prove every embedded schema parses and compiles.
//! A corrupted `include_str!` import or a malformed schema source
//! surfaces here long before any production caller touches the
//! validator.

use jsonschema::{Registry, Resource};
use serde_json::{Value, json};
use specify_error::ValidationStatus;
use specify_schema::{
    CODEX_RULE_JSON_SCHEMA, COMPONENTS_JSON_SCHEMA, EVIDENCE_JSON_SCHEMA, FUSION_JSON_SCHEMA,
    LINT_FINDING_JSON_SCHEMA, LINT_RESULT_JSON_SCHEMA, PLAN_JSON_SCHEMA,
    RESOLVED_CODEX_JSON_SCHEMA, WORKSPACE_MODEL_JSON_SCHEMA, compile_schema, validate_value,
};

#[test]
fn plan_schema_compiles() {
    compile_schema(PLAN_JSON_SCHEMA).expect("plan schema compiles");
}

#[test]
fn evidence_schema_compiles() {
    compile_schema(EVIDENCE_JSON_SCHEMA).expect("evidence schema compiles");
}

#[test]
fn fusion_schema_compiles() {
    compile_schema(FUSION_JSON_SCHEMA).expect("fusion schema compiles");
}

#[test]
fn components_schema_compiles() {
    compile_schema(COMPONENTS_JSON_SCHEMA).expect("components schema compiles");
}

#[test]
fn resolved_codex_schema_compiles() {
    compile_schema(RESOLVED_CODEX_JSON_SCHEMA).expect("resolved codex schema compiles");
}

#[test]
fn codex_rule_schema_compiles() {
    compile_schema(CODEX_RULE_JSON_SCHEMA).expect("codex-rule schema compiles");
}

#[test]
fn lint_finding_schema_compiles() {
    compile_schema(LINT_FINDING_JSON_SCHEMA).expect("lint finding schema compiles");
}

#[test]
fn workspace_model_schema_compiles() {
    compile_schema(WORKSPACE_MODEL_JSON_SCHEMA).expect("workspace-model schema compiles");
}

#[test]
fn workspace_model_schema_accepts_minimal_envelope() {
    let instance = json!({
        "version": 1,
        "project_dir": ".",
        "scan_profile": "consumer",
        "artifact_paths": [],
        "languages": [],
        "files": [],
        "frontmatter": [],
        "markdown_sections": [],
        "markdown_links": [],
        "symlinks": [],
        "skills": [],
        "adapter_manifests": [],
        "marketplace_entries": [],
        "codex_rules": [],
        "text_matches": []
    });
    let summaries = validate_value(
        &instance,
        WORKSPACE_MODEL_JSON_SCHEMA,
        "workspace-model",
        "workspace-model minimal envelope",
    );
    assert!(
        summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
        "minimal envelope must validate; got {summaries:?}"
    );
}

#[test]
fn lint_result_schema_compiles() {
    // The envelope $ref's `finding.schema.json` by relative URI; the
    // standalone `compile_schema` helper has no resource registry, so
    // compile through a registry that pins the finding schema under
    // the same directory the relative ref resolves to.
    let envelope: Value =
        serde_json::from_str(LINT_RESULT_JSON_SCHEMA).expect("lint-result schema parses");
    let finding: Value =
        serde_json::from_str(LINT_FINDING_JSON_SCHEMA).expect("finding schema parses");
    let registry = Registry::new()
        .add(
            "https://github.com/augentic/specify-cli/schemas/lint/finding.schema.json",
            Resource::from_contents(finding),
        )
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry prepares");
    let _validator = jsonschema::options()
        .with_registry(&registry)
        .build(&envelope)
        .expect("lint-result schema compiles with finding $ref resolved");
}

#[test]
fn lint_result_schema_accepts_envelope_with_one_finding() {
    let envelope: Value =
        serde_json::from_str(LINT_RESULT_JSON_SCHEMA).expect("lint-result schema parses");
    let finding: Value =
        serde_json::from_str(LINT_FINDING_JSON_SCHEMA).expect("finding schema parses");
    let registry = Registry::new()
        .add(
            "https://github.com/augentic/specify-cli/schemas/lint/finding.schema.json",
            Resource::from_contents(finding),
        )
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry prepares");
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&envelope)
        .expect("lint-result schema compiles with finding $ref resolved");
    let instance = json!({
        "version": 1,
        "summary": { "critical": 0, "important": 1, "suggestion": 0, "optional": 0 },
        "findings": [{
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
        }]
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "RFC-28 FIND-0001 envelope must validate; errors: {errors:?}");
}

/// Pins the relative-ref form: integrations downstream rely on
/// `finding.schema.json` resolving against the envelope schema's
/// directory rather than an absolute URL.
#[test]
fn lint_result_schema_uses_relative_finding_ref() {
    let envelope: Value =
        serde_json::from_str(LINT_RESULT_JSON_SCHEMA).expect("lint-result schema parses");
    let items_ref = envelope
        .pointer("/properties/findings/items/$ref")
        .and_then(Value::as_str)
        .expect("findings.items.$ref is a string");
    assert_eq!(items_ref, "finding.schema.json");
}

/// Per RFC-32 §"Hint kinds — reserved", reserved kinds are
/// shape-validated by this schema with no execution semantics. A
/// minimal codex-rule frontmatter that declares each reserved kind
/// must round-trip cleanly so RFC-28 exporters accept files awaiting
/// implementation.
#[test]
fn codex_rule_schema_accepts_each_reserved_hint_kind() {
    let reserved = [
        "unique",
        "reference-resolves",
        "set-coverage",
        "cardinality",
        "constant-eq",
        "set-eq",
        "content-digest-eq",
        "namespace-owner",
    ];
    for kind in reserved {
        let instance = json!({
            "id": "UNI-014",
            "title": "Reserved-kind smoke fixture",
            "severity": "important",
            "trigger": "Reserved hint kind smoke fixture for RFC-32 §Hint kinds — reserved.",
            "deterministic_hints": [{
                "kind": kind,
                "value": "placeholder"
            }]
        });
        let summaries = validate_value(
            &instance,
            CODEX_RULE_JSON_SCHEMA,
            "codex-rule",
            "codex rule fixture per reserved kind",
        );
        assert!(
            summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
            "kind {kind} must validate; got {summaries:?}"
        );
    }
}
