//! Smoke tests that prove every embedded schema parses and compiles.
//! A corrupted `include_str!` import or a malformed schema source
//! surfaces here long before any production caller touches the
//! validator.

use jsonschema::{Registry, Resource};
use serde_json::{Value, json};
use specify_schema::{
    BUILD_REPORT_JSON_SCHEMA, BUILD_REQUEST_JSON_SCHEMA, COMPONENTS_JSON_SCHEMA,
    DIAGNOSTIC_JSON_SCHEMA, DIAGNOSTIC_REPORT_JSON_SCHEMA, EVIDENCE_JSON_SCHEMA,
    MARKETPLACE_JSON_SCHEMA, PLAN_JSON_SCHEMA, PROVENANCE_JSON_SCHEMA, RESOLVED_RULES_JSON_SCHEMA,
    RULE_JSON_SCHEMA, SCENARIO_JSON_SCHEMA, SKILL_JSON_SCHEMA, SLICE_MODEL_JSON_SCHEMA,
    SYNTHESIS_JSON_SCHEMA, ValidationStatus, WORKSPACE_MODEL_JSON_SCHEMA, compile_schema,
    validate_value,
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
fn provenance_schema_compiles() {
    compile_schema(PROVENANCE_JSON_SCHEMA).expect("provenance schema compiles");
}

#[test]
fn slice_model_schema_compiles() {
    compile_schema(SLICE_MODEL_JSON_SCHEMA).expect("slice model schema compiles");
}

/// The single slice-model schema validates an agent synthesis response
/// `model` (kernel-owned + header fields omitted) — proving the
/// optional-field design that lets one schema serve both the response
/// and the persisted file.
#[test]
fn slice_model_schema_accepts_agent_response_without_kernel_fields() {
    let instance = json!({
        "requirements": [{
            "title": "Request password reset",
            "agreement": "agreed",
            "claims": [
                { "source": "docs", "id": "password-reset.request", "kind": "requirement" }
            ],
            "statement": "The system lets a user request a reset link."
        }],
        "tasks": []
    });
    let summaries = validate_value(
        &instance,
        SLICE_MODEL_JSON_SCHEMA,
        "slice-model",
        "agent response model omits kernel-owned and header fields",
    );
    assert!(
        summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
        "agent response without kernel fields must validate; got {summaries:?}"
    );
}

/// Compile the synthesis schema through a registry that pins the model
/// schema under its `$id`, so the response's relative `model` `$ref`
/// resolves (mirrors `lint_result_schema_compiles`).
fn synthesis_validator() -> jsonschema::Validator {
    let synthesis: Value =
        serde_json::from_str(SYNTHESIS_JSON_SCHEMA).expect("synthesis schema parses");
    let model: Value = serde_json::from_str(SLICE_MODEL_JSON_SCHEMA).expect("model schema parses");
    let registry = Registry::new()
        .add(
            "https://github.com/augentic/specify-cli/schemas/slice/model.schema.json",
            Resource::from_contents(model),
        )
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry prepares");
    jsonschema::options()
        .with_registry(&registry)
        .build(&synthesis)
        .expect("synthesis schema compiles with model $ref resolved")
}

#[test]
fn synthesis_schema_compiles() {
    let _validator = synthesis_validator();
}

/// The RFC-29c §"Synthesis response" worked example validates against
/// the synthesis schema with the model `$ref` resolved.
#[test]
fn synthesis_schema_accepts_rfc_response_example() {
    let validator = synthesis_validator();
    let instance = json!({
        "kind": "response",
        "version": 1,
        "slice": "identity-service",
        "model": {
            "requirements": [{
                "title": "Request password reset",
                "unit": "password-reset",
                "agreement": "agreed",
                "claims": [
                    { "source": "docs", "id": "password-reset.request", "kind": "requirement" },
                    { "source": "legacy", "id": "users.password-reset.request", "kind": "example" }
                ],
                "statement": "The system lets a registered user request a password reset link by email.",
                "scenarios": [
                    "Given a registered email, when the user requests a reset, then the system accepts it."
                ]
            }],
            "tasks": [{
                "id": "TASK-001",
                "text": "Implement password reset request handling.",
                "satisfies": ["REQ-001"]
            }]
        },
        "artifacts": {
            "proposal": "# Password reset\n…",
            "design": "# Design\n…",
            "tasks": "# Tasks\n- [ ] TASK-001 …",
            "specs": [{
                "unit": "password-reset",
                "content": "## Request password reset\nThe system lets a registered user…"
            }]
        }
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "RFC synthesis response must validate; errors: {errors:?}");
}

#[test]
fn components_schema_compiles() {
    compile_schema(COMPONENTS_JSON_SCHEMA).expect("components schema compiles");
}

#[test]
fn resolved_codex_schema_compiles() {
    compile_schema(RESOLVED_RULES_JSON_SCHEMA).expect("resolved codex schema compiles");
}

#[test]
fn codex_rule_schema_compiles() {
    compile_schema(RULE_JSON_SCHEMA).expect("codex-rule schema compiles");
}

#[test]
fn skill_schema_compiles() {
    compile_schema(SKILL_JSON_SCHEMA).expect("skill schema compiles");
}

#[test]
fn scenario_schema_compiles() {
    compile_schema(SCENARIO_JSON_SCHEMA).expect("scenario schema compiles");
}

#[test]
fn marketplace_schema_compiles() {
    compile_schema(MARKETPLACE_JSON_SCHEMA).expect("marketplace schema compiles");
}

#[test]
fn lint_finding_schema_compiles() {
    compile_schema(DIAGNOSTIC_JSON_SCHEMA).expect("lint finding schema compiles");
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
        "rule_index": [],
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
        serde_json::from_str(DIAGNOSTIC_REPORT_JSON_SCHEMA).expect("lint-result schema parses");
    let finding: Value =
        serde_json::from_str(DIAGNOSTIC_JSON_SCHEMA).expect("finding schema parses");
    let registry = Registry::new()
        .add(
            "https://github.com/augentic/specify-cli/schemas/diagnostics/diagnostic.schema.json",
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
        serde_json::from_str(DIAGNOSTIC_REPORT_JSON_SCHEMA).expect("lint-result schema parses");
    let finding: Value =
        serde_json::from_str(DIAGNOSTIC_JSON_SCHEMA).expect("finding schema parses");
    let registry = Registry::new()
        .add(
            "https://github.com/augentic/specify-cli/schemas/diagnostics/diagnostic.schema.json",
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
    assert!(errors.is_empty(), "FIND-0001 envelope must validate; errors: {errors:?}");
}

/// Pins the relative-ref form: integrations downstream rely on
/// `diagnostic.schema.json` resolving against the envelope schema's
/// directory rather than an absolute URL.
#[test]
fn diagnostic_report_schema_uses_relative_diagnostic_ref() {
    let envelope: Value = serde_json::from_str(DIAGNOSTIC_REPORT_JSON_SCHEMA)
        .expect("diagnostic-report schema parses");
    let items_ref = envelope
        .pointer("/properties/findings/items/$ref")
        .and_then(Value::as_str)
        .expect("findings.items.$ref is a string");
    assert_eq!(items_ref, "diagnostic.schema.json");
}

#[test]
fn build_request_schema_compiles() {
    compile_schema(BUILD_REQUEST_JSON_SCHEMA).expect("build-request schema compiles");
}

/// The RFC-29d §"Build request" worked example validates.
#[test]
fn build_request_schema_accepts_rfc_example() {
    let instance = json!({
        "version": 1,
        "slice": "identity-service",
        "project-dir": "/workspace/.specify/workspace/identity-service",
        "inputs": {
            "root": "/workspace/.specify/slices/identity-service",
            "artifacts": {
                "proposal": "proposal.md",
                "specs": ["specs/identity/spec.md"],
                "design": "design.md",
                "tasks": "tasks.md",
                "additional": ["tokens.yaml"]
            }
        }
    });
    let summaries = validate_value(
        &instance,
        BUILD_REQUEST_JSON_SCHEMA,
        "build-request",
        "RFC-29d build request example",
    );
    assert!(
        summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
        "RFC build request must validate; got {summaries:?}"
    );
}

/// Compile the build-report schema through a registry that pins the
/// diagnostic schema under its `$id`, so the report's relative
/// `../diagnostics/diagnostic.schema.json` `findings[]` `$ref` resolves
/// (mirrors `lint_result_schema_compiles`).
fn build_report_validator() -> jsonschema::Validator {
    let report: Value =
        serde_json::from_str(BUILD_REPORT_JSON_SCHEMA).expect("build-report schema parses");
    let diagnostic: Value =
        serde_json::from_str(DIAGNOSTIC_JSON_SCHEMA).expect("diagnostic schema parses");
    let registry = Registry::new()
        .add(
            "https://github.com/augentic/specify-cli/schemas/diagnostics/diagnostic.schema.json",
            Resource::from_contents(diagnostic),
        )
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry prepares");
    jsonschema::options()
        .with_registry(&registry)
        .build(&report)
        .expect("build-report schema compiles with diagnostic $ref resolved")
}

#[test]
fn build_report_schema_compiles() {
    let _validator = build_report_validator();
}

/// The RFC-29d §"Build report" success example validates.
#[test]
fn build_report_schema_accepts_success() {
    let validator = build_report_validator();
    let instance = json!({
        "version": 1,
        "slice": "identity-service",
        "target": "omnia@v1",
        "status": "success",
        "findings": []
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "success report must validate; errors: {errors:?}");
}

/// The RFC-29d §"Build report" failure (no findings) example validates.
#[test]
fn build_report_schema_accepts_failure_without_findings() {
    let validator = build_report_validator();
    let instance = json!({
        "version": 1,
        "slice": "identity-service",
        "target": "omnia@v1",
        "status": "failure",
        "findings": []
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "failure report must validate; errors: {errors:?}");
}

/// The RFC-29d §"Build report" failure-with-findings example validates,
/// proving the relative diagnostic `$ref` accepts a full RFC-28 finding.
#[test]
fn build_report_schema_accepts_failure_with_findings() {
    let validator = build_report_validator();
    let instance = json!({
        "version": 1,
        "slice": "identity-contracts",
        "target": "contracts@v1",
        "status": "failure",
        "findings": [{
            "id": "DIAG-0001",
            "rule-id": "contract.id-unique",
            "title": "Duplicate info.x-specify-id across baseline",
            "severity": "critical",
            "source": "tool",
            "kind": "violation",
            "target-adapter": "contracts",
            "slice": "identity-contracts",
            "artifact": "contracts",
            "location": {
                "path": "contracts/http/user-api.yaml"
            },
            "evidence": {
                "kind": "structured",
                "summary": "x-specify-id user-api collides with legacy-api.yaml",
                "data": {
                    "detail": "info.x-specify-id user-api also present on contracts/http/legacy-api.yaml"
                }
            },
            "impact": "Downstream consumers cannot resolve a unique contract id.",
            "remediation": "Rename or remove the duplicate id before merge.",
            "fingerprint": "sha256:a2e95674f838eb042eba78e16239f32199def3ca976e29499f8275beb30225e4"
        }]
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "failure-with-findings report must validate; errors: {errors:?}");
}

/// Per the standards-layer contract §"Hint kinds — reserved", reserved kinds are
/// shape-validated by this schema with no execution semantics. A
/// minimal codex-rule frontmatter that declares each reserved kind
/// must round-trip cleanly so rules exporters accept files awaiting
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
            "trigger": "Reserved hint kind smoke fixture.",
            "deterministic_hints": [{
                "kind": kind,
                "value": "placeholder"
            }]
        });
        let summaries =
            validate_value(&instance, RULE_JSON_SCHEMA, "rule", "rule fixture per reserved kind");
        assert!(
            summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
            "kind {kind} must validate; got {summaries:?}"
        );
    }
}
