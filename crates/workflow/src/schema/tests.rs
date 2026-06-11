use super::*;

/// The `UNI-014` example for the `ResolvedRules` export
/// validates cleanly against the resolved-codex schema.
#[test]
fn resolved_codex_accepts_example() {
    let instance = serde_json::json!({
        "version": 1,
        "target-adapter": "omnia",
        "source-adapters": ["typescript"],
        "rules": [
            {
                "rule-id": "UNI-014",
                "title": "Hardcoded Configuration",
                "severity": "important",
                "trigger": "Generated code embeds environment-specific configuration instead of routing it through declared configuration.",
                "lint-mode": "hybrid",
                "origin": "shared",
                "path-root": "rules-root",
                "path": "adapters/shared/rules/universal/hardcoded-configuration.md",
                "applicability": {
                    "adapters": ["omnia"],
                    "languages": ["rust"],
                    "artifacts": ["code"]
                },
                "rule-hints": [
                    {
                        "kind": "regex",
                        "value": "https?://",
                        "description": "Literal URL in generated code."
                    }
                ],
                "references": [
                    {
                        "label": "Omnia guardrails",
                        "path": "adapters/targets/omnia/references/guardrails.md"
                    }
                ],
                "body": "## Rule\n\nConfiguration values that vary between deployments must not be hardcoded in generated code.\n",
                "deprecated": null
            }
        ]
    });
    let validator =
        compile_schema(RESOLVED_RULES_JSON_SCHEMA).expect("resolved codex schema compiles");
    let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "UNI-014 example must validate; errors: {errors:?}");
}

/// The `FIND-0001` example for structured lint findings
/// schema validates cleanly against the finding schema. The
/// fingerprint placeholder `sha256:...` from the contract is
/// replaced with a deterministic 64-hex-char digest so the
/// fingerprint pattern check passes.
#[test]
fn review_finding_accepts_example() {
    let instance = serde_json::json!({
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
    });
    let validator = compile_schema(DIAGNOSTIC_JSON_SCHEMA).expect("review finding schema compiles");
    let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "FIND-0001 example must validate; errors: {errors:?}");
}

/// The rule frontmatter example for codex file shape
/// validates cleanly against the vendored codex-rule schema.
#[test]
fn codex_rule_accepts_example() {
    let instance = serde_json::json!({
        "id": "UNI-014",
        "title": "Hardcoded Configuration",
        "severity": "important",
        "trigger": "Generated code embeds environment-specific configuration instead of routing it through declared configuration.",
        "applicability": {
            "adapters": ["omnia"],
            "languages": ["rust"],
            "artifacts": ["code"]
        },
        "lint_mode": "hybrid",
        "rule_hints": [
            {
                "kind": "regex",
                "value": "https?://",
                "description": "Literal URL in generated code."
            }
        ]
    });
    let validator = compile_schema(RULE_JSON_SCHEMA).expect("codex-rule schema compiles");
    let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "UNI-014 frontmatter must validate; errors: {errors:?}");
}

/// An empty evidence directory (or missing one) passes — empty
/// extraction is a legal slice state per workflow §Extraction
/// reliability.
#[test]
fn missing_evidence_dir_is_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    validate_evidence_dir(dir.path()).expect("missing evidence dir is ok");
}

/// The embedded proposal envelope schema compiles.
#[test]
fn proposal_schema_compiles() {
    compile_schema(PROPOSAL_JSON_SCHEMA).expect("proposal schema compiles");
}

/// The multi-source `kind: request` envelope example validates.
#[test]
fn proposal_accepts_rfc_request() {
    let request = r#"
version: 1
kind: request
projects:
  - name: identity-contracts
    target: contracts@v1
    description: "Versioned API contracts crate for the identity domain."
  - name: identity-service
    target: omnia@v1
    description: "Omnia identity service implementing auth and password flows."
leads:
  - source: docs
    lead: identity-api
    synopsis: "Identity API contract for authentication and account access."
  - source: legacy
    lead: identity-api
    synopsis: "Legacy identity endpoints."
  - source: docs
    lead: password-reset
    synopsis: "Users can request a password reset email."
  - source: legacy
    lead: reset-password
    synopsis: "Legacy reset-password flow."
"#;
    validate_proposal_json(request).expect("RFC request example validates");
}

/// The N=1 degenerate `kind: response` envelope example validates.
#[test]
fn proposal_accepts_rfc_n1_response() {
    let response = r"
version: 1
kind: response
slices:
  - name: fix-typo
    sources:
      - { source: intent, lead: fix-typo }
";
    validate_proposal_json(response).expect("RFC N=1 response example validates");
}

/// The multi-source fan-out `kind: response` envelope example validates.
#[test]
fn proposal_accepts_rfc_fanout_response() {
    let response = r#"
version: 1
kind: response
slices:
  - name: identity-contracts
    sources:
      - { source: docs, lead: identity-api }
      - { source: legacy, lead: identity-api }
    project: identity-contracts
    rationale: "identity API surface matched by shared slug across docs + legacy"
  - name: identity-service
    sources:
      - { source: docs, lead: identity-api }
      - { source: legacy, lead: identity-api }
    project: identity-service
    depends-on: [identity-contracts]
  - name: password-reset
    sources:
      - { source: docs, lead: password-reset }
      - { source: legacy, lead: reset-password }
    project: identity-service
    rationale: "password-reset (docs) and reset-password (legacy) are the same flow by summary judgment"
"#;
    validate_proposal_json(response).expect("RFC fan-out response example validates");
}

/// The build request example validates.
#[test]
fn build_request_accepts_rfc_example() {
    let request = r#"{
            "version": 1,
            "slice": "identity-service",
            "project-dir": "/workspace/.specify/workspace/identity-service",
            "inputs": {
                "root": "/workspace/.specify/slices/identity-service",
                "artifacts": {
                    "proposal": "proposal.md",
                    "design": "design.md",
                    "tasks": "tasks.md",
                    "specs": ["specs/identity/spec.md"],
                    "additional": ["tokens.yaml"]
                }
            }
        }"#;
    validate_build_request_json(request).expect("RFC build request validates");
}

/// A request missing the required `inputs` block is rejected.
#[test]
fn build_request_rejects_malformed() {
    let request = r#"{"version": 1, "slice": "identity-service", "project-dir": "/w"}"#;
    match validate_build_request_json(request) {
        Err(Error::Validation { code, .. }) => assert_eq!(code, "target-build-request-schema"),
        other => panic!("expected target-build-request-schema, got {other:?}"),
    }
}

/// A failure report carrying a full finding validates,
/// proving the relative diagnostic `$ref` resolves through the
/// registry.
#[test]
fn build_report_accepts_failure() {
    let report = r#"{
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
                "location": { "path": "contracts/http/user-api.yaml" },
                "evidence": {
                    "kind": "structured",
                    "summary": "x-specify-id user-api collides with legacy-api.yaml",
                    "data": { "detail": "duplicate id" }
                },
                "impact": "Downstream consumers cannot resolve a unique contract id.",
                "remediation": "Rename or remove the duplicate id before merge.",
                "fingerprint": "sha256:a2e95674f838eb042eba78e16239f32199def3ca976e29499f8275beb30225e4"
            }]
        }"#;
    validate_build_report_json(report).expect("failure-with-finding report validates");
}

/// A report with an out-of-enum `status` is rejected.
#[test]
fn build_report_rejects_malformed() {
    let report = r#"{
            "version": 1,
            "slice": "identity-service",
            "target": "omnia@v1",
            "status": "partial",
            "findings": []
        }"#;
    match validate_build_report_json(report) {
        Err(Error::Validation { code, .. }) => assert_eq!(code, "target-build-report-schema"),
        other => panic!("expected target-build-report-schema, got {other:?}"),
    }
}

/// A malformed envelope (missing `kind`, which leaves it matching
/// neither `oneOf` branch) is rejected with the `proposal-schema`
/// code.
#[test]
fn proposal_rejects_malformed_envelope() {
    let malformed = r"
version: 1
slices:
  - name: orphan
    sources:
      - { source: intent, lead: orphan }
";
    match validate_proposal_json(malformed) {
        Err(Error::Validation { code, .. }) => assert_eq!(code, "proposal-schema"),
        other => panic!("expected proposal-schema validation error, got {other:?}"),
    }
}
