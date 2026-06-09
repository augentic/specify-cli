//! Smoke tests that prove every embedded schema parses and compiles.
//! A corrupted `include_str!` import or a malformed schema source
//! surfaces here long before any production caller touches the
//! validator.

use jsonschema::{Registry, Resource};
use serde_json::{Value, json};
use specify_schema::{
    ADAPTER_JSON_SCHEMA, BUILD_REPORT_JSON_SCHEMA, BUILD_REQUEST_JSON_SCHEMA,
    COMPONENTS_JSON_SCHEMA, DECISION_JSON_SCHEMA, DIAGNOSTIC_JSON_SCHEMA,
    DIAGNOSTIC_REPORT_JSON_SCHEMA, EVIDENCE_JSON_SCHEMA, FRAMEWORK_JSON_SCHEMA, LEAD_JSON_SCHEMA,
    MARKETPLACE_JSON_SCHEMA, PLAN_JSON_SCHEMA, PROPOSAL_JSON_SCHEMA, PROVENANCE_JSON_SCHEMA,
    RESOLVED_RULES_JSON_SCHEMA, RULE_JSON_SCHEMA, SCENARIO_JSON_SCHEMA, SKILL_JSON_SCHEMA,
    SLICE_MODEL_JSON_SCHEMA, SOURCE_JSON_SCHEMA, SYNTHESIS_JSON_SCHEMA, TARGET_JSON_SCHEMA,
    TOOL_JSON_SCHEMA, TOOL_SIDECAR_JSON_SCHEMA, TOPOLOGY_LOCK_JSON_SCHEMA, ValidationStatus,
    WORKSPACE_MODEL_JSON_SCHEMA, compile_schema, validate_value,
};

#[test]
fn plan_schema_compiles() {
    compile_schema(PLAN_JSON_SCHEMA).expect("plan schema compiles");
}

#[test]
fn lead_schema_compiles() {
    compile_schema(LEAD_JSON_SCHEMA).expect("lead schema compiles");
}

#[test]
fn proposal_schema_compiles() {
    compile_schema(PROPOSAL_JSON_SCHEMA).expect("proposal schema compiles");
}

#[test]
fn topology_lock_schema_compiles() {
    compile_schema(TOPOLOGY_LOCK_JSON_SCHEMA).expect("topology-lock schema compiles");
}

#[test]
fn adapter_schema_compiles() {
    compile_schema(ADAPTER_JSON_SCHEMA).expect("adapter schema compiles");
}

#[test]
fn source_schema_compiles() {
    compile_schema(SOURCE_JSON_SCHEMA).expect("source schema compiles");
}

#[test]
fn target_schema_compiles() {
    compile_schema(TARGET_JSON_SCHEMA).expect("target schema compiles");
}

#[test]
fn tool_schema_compiles() {
    compile_schema(TOOL_JSON_SCHEMA).expect("tool schema compiles");
}

#[test]
fn tool_sidecar_schema_compiles() {
    compile_schema(TOOL_SIDECAR_JSON_SCHEMA).expect("tool-sidecar schema compiles");
}

/// `cache-meta.schema.json` ships on disk but is not embedded as a
/// constant. Compile it straight from disk so a malformed edit fails
/// in CI even without an `include_str!` binding.
#[test]
fn cache_meta_schema_compiles_from_disk() {
    let source = include_str!("../../../schemas/cache-meta.schema.json");
    compile_schema(source).expect("cache-meta schema compiles");
}

/// `context-lock.schema.json` ships on disk but is not embedded as a
/// constant; compile it straight from disk (see
/// [`cache_meta_schema_compiles_from_disk`]).
#[test]
fn context_lock_schema_compiles_from_disk() {
    let source = include_str!("../../../schemas/context-lock.schema.json");
    compile_schema(source).expect("context-lock schema compiles");
}

/// Every embedded schema constant must byte-match its on-disk source
/// (REVIEW.md A11). `include_str!` binds at compile time, so this guards
/// against a constant pointing at a stale or duplicated copy: each entry
/// re-reads the canonical workspace file at runtime and asserts equality.
#[test]
fn embedded_schemas_match_on_disk_sources() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crates/schema has a repo root two levels up");
    let pairs: &[(&str, &str, &str)] = &[
        ("ADAPTER_JSON_SCHEMA", ADAPTER_JSON_SCHEMA, "schemas/adapter.schema.json"),
        ("SOURCE_JSON_SCHEMA", SOURCE_JSON_SCHEMA, "schemas/source.schema.json"),
        ("TARGET_JSON_SCHEMA", TARGET_JSON_SCHEMA, "schemas/target.schema.json"),
        ("TOOL_JSON_SCHEMA", TOOL_JSON_SCHEMA, "schemas/tool.schema.json"),
        ("TOOL_SIDECAR_JSON_SCHEMA", TOOL_SIDECAR_JSON_SCHEMA, "schemas/tool-sidecar.schema.json"),
        ("PLAN_JSON_SCHEMA", PLAN_JSON_SCHEMA, "schemas/plan/plan.schema.json"),
        ("EVIDENCE_JSON_SCHEMA", EVIDENCE_JSON_SCHEMA, "schemas/evidence.schema.json"),
        ("LEAD_JSON_SCHEMA", LEAD_JSON_SCHEMA, "schemas/discovery/lead.schema.json"),
        ("PROPOSAL_JSON_SCHEMA", PROPOSAL_JSON_SCHEMA, "schemas/discovery/proposal.schema.json"),
        ("SLICE_MODEL_JSON_SCHEMA", SLICE_MODEL_JSON_SCHEMA, "schemas/slice/model.schema.json"),
        ("SYNTHESIS_JSON_SCHEMA", SYNTHESIS_JSON_SCHEMA, "schemas/slice/synthesis.schema.json"),
        ("PROVENANCE_JSON_SCHEMA", PROVENANCE_JSON_SCHEMA, "schemas/slice/provenance.schema.json"),
        (
            "TOPOLOGY_LOCK_JSON_SCHEMA",
            TOPOLOGY_LOCK_JSON_SCHEMA,
            "schemas/topology-lock.schema.json",
        ),
        (
            "COMPONENTS_JSON_SCHEMA",
            COMPONENTS_JSON_SCHEMA,
            "schemas/design-system/components.schema.json",
        ),
        (
            "RESOLVED_RULES_JSON_SCHEMA",
            RESOLVED_RULES_JSON_SCHEMA,
            "schemas/rules/resolved.schema.json",
        ),
        ("RULE_JSON_SCHEMA", RULE_JSON_SCHEMA, "schemas/rules/rule.schema.json"),
        (
            "DIAGNOSTIC_JSON_SCHEMA",
            DIAGNOSTIC_JSON_SCHEMA,
            "schemas/diagnostics/diagnostic.schema.json",
        ),
        (
            "DIAGNOSTIC_REPORT_JSON_SCHEMA",
            DIAGNOSTIC_REPORT_JSON_SCHEMA,
            "schemas/diagnostics/diagnostic-report.schema.json",
        ),
        (
            "WORKSPACE_MODEL_JSON_SCHEMA",
            WORKSPACE_MODEL_JSON_SCHEMA,
            "schemas/lint/workspace-model.schema.json",
        ),
        ("SKILL_JSON_SCHEMA", SKILL_JSON_SCHEMA, "schemas/authoring/skill.schema.json"),
        ("SCENARIO_JSON_SCHEMA", SCENARIO_JSON_SCHEMA, "schemas/authoring/scenario.schema.json"),
        (
            "MARKETPLACE_JSON_SCHEMA",
            MARKETPLACE_JSON_SCHEMA,
            "schemas/authoring/marketplace.schema.json",
        ),
        ("FRAMEWORK_JSON_SCHEMA", FRAMEWORK_JSON_SCHEMA, "schemas/authoring/framework.schema.json"),
        (
            "BUILD_REQUEST_JSON_SCHEMA",
            BUILD_REQUEST_JSON_SCHEMA,
            "schemas/target/build-request.schema.json",
        ),
        (
            "BUILD_REPORT_JSON_SCHEMA",
            BUILD_REPORT_JSON_SCHEMA,
            "schemas/target/build-report.schema.json",
        ),
    ];
    for (name, embedded, relative) in pairs {
        let on_disk = std::fs::read_to_string(repo_root.join(relative))
            .unwrap_or_else(|err| panic!("read {relative} for {name}: {err}"));
        assert_eq!(
            *embedded, on_disk,
            "{name} embed diverges from on-disk {relative}; re-run the embed or fix the file"
        );
    }
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
fn slice_model_accepts_no_kernel() {
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

/// The synthesis-response worked example validates against the
/// synthesis schema with the model `$ref` resolved.
#[test]
fn synthesis_accepts_example() {
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
fn decision_schema_compiles() {
    compile_schema(DECISION_JSON_SCHEMA).expect("decision schema compiles");
}

/// The slice-authored Decision Record example validates without
/// the engine-stamped `id` / `slice` / `date` fields — proving the
/// optional-field design that lets one schema serve both the slice form
/// and the persisted baseline form.
#[test]
fn decision_accepts_slice_form() {
    let instance = json!({
        "slug": "identity-store-postgres",
        "status": "accepted",
        "supersedes": ["DEC-0003"],
        "related": ["REQ-001", "REQ-014"]
    });
    let summaries = validate_value(
        &instance,
        DECISION_JSON_SCHEMA,
        "decision",
        "slice-authored decision omits engine-stamped fields",
    );
    assert!(
        summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
        "slice-authored decision must validate; got {summaries:?}"
    );
}

/// The promoted baseline Decision Record example validates with
/// the engine-stamped header fields present.
#[test]
fn decision_accepts_baseline_form() {
    let instance = json!({
        "id": "DEC-0007",
        "slug": "identity-store-postgres",
        "status": "accepted",
        "slice": "identity-service",
        "date": "2026-06-02",
        "supersedes": ["DEC-0003"],
        "related": ["REQ-001", "REQ-014"]
    });
    let summaries = validate_value(
        &instance,
        DECISION_JSON_SCHEMA,
        "decision",
        "promoted baseline decision carries engine-stamped fields",
    );
    assert!(
        summaries.iter().all(|s| matches!(s.status, ValidationStatus::Pass)),
        "promoted baseline decision must validate; got {summaries:?}"
    );
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
fn framework_schema_compiles() {
    compile_schema(FRAMEWORK_JSON_SCHEMA).expect("framework schema compiles");
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
fn workspace_model_accepts_minimal() {
    let instance = json!({
        "version": 1,
        "project_dir": ".",
        "scan_profile": "project",
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
fn lint_result_accepts_one_finding() {
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
fn diagnostic_report_relative_ref() {
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

/// The build-request worked example validates.
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
        "build request example",
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

/// The build-report success example validates.
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

/// The build-report failure (no findings) example validates.
#[test]
fn build_report_failure_no_findings() {
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

/// A success report with `outputs[]` validates, proving the new
/// `buildOutput` `$def` and `platform` enum resolve correctly.
#[test]
fn build_report_success_outputs() {
    let validator = build_report_validator();
    let instance = json!({
        "version": 1,
        "slice": "identity-service",
        "target": "vectis@v1",
        "status": "success",
        "findings": [],
        "outputs": [
            { "platform": "core", "path": "shared/src/app.rs" },
            { "platform": "ios", "path": "iOS/MyApp/ContentView.swift" },
            { "platform": "android", "path": "Android/app/src/main/kotlin/Main.kt" }
        ]
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "success report with outputs must validate; errors: {errors:?}");
}

/// A report without `outputs` validates (backward compatibility).
#[test]
fn build_report_no_outputs() {
    let validator = build_report_validator();
    let instance = json!({
        "version": 1,
        "slice": "identity-service",
        "target": "omnia@v1",
        "status": "success",
        "findings": []
    });
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(errors.is_empty(), "report without outputs must validate; errors: {errors:?}");
}

/// The build-report failure-with-findings example validates, proving
/// the relative diagnostic `$ref` accepts a full finding.
#[test]
fn build_report_failure_with_findings() {
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
fn codex_rule_accepts_reserved_kinds() {
    let reserved = [
        "unique",
        "reference-resolves",
        "set-coverage",
        "cardinality",
        "constant-eq",
        "set-eq",
        "content-digest-eq",
    ];
    for kind in reserved {
        let instance = json!({
            "id": "UNI-014",
            "title": "Reserved-kind smoke fixture",
            "severity": "important",
            "trigger": "Reserved hint kind smoke fixture.",
            "rule_hints": [{
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
