//! Golden tests for the the workflow contract JSON Schemas shipped under
//! `cli/schemas/`: `adapter.schema.json`, `source.schema.json`,
//! `target.schema.json`, `evidence.schema.json`, and
//! `discovery/lead.schema.json`. Each schema gets a "valid"
//! fixture that must validate cleanly plus a small set of "invalid"
//! fixtures (missing required field, wrong enum value, wrong type)
//! that the schema must reject.
//!
//! Fixtures are inlined as `&str` so a fixture-vs-rule mismatch is
//! diff-visible in one file; if this list outgrows the file, move the
//! YAML/JSON bodies under `tests/schemas/<schema>/{valid,invalid}/*`.

use std::path::PathBuf;

use jsonschema::Validator;
use serde_json::Value as JsonValue;

fn schemas_root() -> PathBuf {
    // `crates/domain/tests/` -> `crates/domain/` -> `crates/` -> repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

fn load(path: &str) -> Validator {
    let raw = std::fs::read_to_string(schemas_root().join(path))
        .unwrap_or_else(|err| panic!("read {path}: {err}"));
    let schema: JsonValue =
        serde_json::from_str(&raw).unwrap_or_else(|err| panic!("{path} is valid JSON: {err}"));
    jsonschema::validator_for(&schema)
        .unwrap_or_else(|err| panic!("{path} compiles as JSON Schema: {err}"))
}

fn yaml(input: &str) -> JsonValue {
    serde_saphyr::from_str(input).expect("fixture parses as YAML")
}

fn assert_valid(validator: &Validator, instance: &JsonValue, ctx: &str) {
    let errors: Vec<String> =
        validator.iter_errors(instance).map(|e| format!("{}: {e}", e.instance_path())).collect();
    assert!(errors.is_empty(), "{ctx}: should validate cleanly; got {errors:#?}");
}

fn assert_invalid(validator: &Validator, instance: &JsonValue, ctx: &str) {
    let count = validator.iter_errors(instance).count();
    assert!(count > 0, "{ctx}: schema should reject the fixture but did not");
}

// --- adapter.schema.json --------------------------------------------

const PLUGIN_VALID_SOURCE: &str = r"
name: code-typescript
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
description: Extracts behavioural evidence from TypeScript codebases.
";

const PLUGIN_VALID_TARGET: &str = r"
name: omnia
version: 1
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
description: Omnia Rust WASM target adapter.
";

const PLUGIN_INVALID_NO_AXIS: &str = r"
name: code-typescript
version: 1
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";

const PLUGIN_INVALID_BAD_AXIS: &str = r"
name: code-typescript
version: 1
axis: lens
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";

const PLUGIN_INVALID_NAME_NOT_KEBAB: &str = r"
name: CodeTypeScript
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";

const PLUGIN_INVALID_VERSION_FLOAT: &str = r"
name: code-typescript
version: 1.5
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";

#[test]
fn plugin_accepts_source_and_target_shapes() {
    let v = load("adapter.schema.json");
    assert_valid(&v, &yaml(PLUGIN_VALID_SOURCE), "plugin/source");
    assert_valid(&v, &yaml(PLUGIN_VALID_TARGET), "plugin/target");
}

#[test]
fn plugin_rejects_axis_and_primitives() {
    let v = load("adapter.schema.json");
    assert_invalid(&v, &yaml(PLUGIN_INVALID_NO_AXIS), "plugin/no-axis");
    assert_invalid(&v, &yaml(PLUGIN_INVALID_BAD_AXIS), "plugin/bad-axis");
    assert_invalid(&v, &yaml(PLUGIN_INVALID_NAME_NOT_KEBAB), "plugin/name-not-kebab");
    assert_invalid(&v, &yaml(PLUGIN_INVALID_VERSION_FLOAT), "plugin/version-float");
}

// --- source.schema.json --------------------------------------------

const SOURCE_INVALID_AXIS_TARGET: &str = r"
name: code-typescript
version: 1
axis: target
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";

const SOURCE_INVALID_EXTRA_BRIEF: &str = r"
name: code-typescript
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
  shape: briefs/shape.md
";

const SOURCE_INVALID_MISSING_BRIEF: &str = r"
name: code-typescript
version: 1
axis: source
briefs:
  survey: briefs/survey.md
";

#[test]
fn source_accepts_canonical_shape() {
    let v = load("source.schema.json");
    assert_valid(&v, &yaml(PLUGIN_VALID_SOURCE), "source/valid");
}

#[test]
fn source_rejects_axis_and_brief_violations() {
    // With the dedicated `operations[]` field collapsed (review 1.A1),
    // brief-key validity is the only thing closing the operation set.
    // Cover both "extra key under briefs:" and "required brief
    // missing" to pin that surface.
    let v = load("source.schema.json");
    assert_invalid(&v, &yaml(SOURCE_INVALID_AXIS_TARGET), "source/axis-target");
    assert_invalid(&v, &yaml(SOURCE_INVALID_EXTRA_BRIEF), "source/extra-brief");
    assert_invalid(&v, &yaml(SOURCE_INVALID_MISSING_BRIEF), "source/missing-brief");
}

// --- target.schema.json --------------------------------------------

const TARGET_INVALID_AXIS_SOURCE: &str = r"
name: omnia
version: 1
axis: source
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
";

const TARGET_INVALID_BRIEFS_INCLUDE_EXTRACT: &str = r"
name: omnia
version: 1
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
  extract: briefs/extract.md
";

const TARGET_INVALID_MISSING_MERGE_BRIEF: &str = r"
name: omnia
version: 1
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
";

#[test]
fn target_accepts_canonical_shape() {
    let v = load("target.schema.json");
    assert_valid(&v, &yaml(PLUGIN_VALID_TARGET), "target/valid");
}

#[test]
fn target_rejects_axis_and_brief_violations() {
    // With the dedicated `operations[]` field collapsed (review 1.A1),
    // the `briefs.*` key set is what closes the target operation set.
    // Cover an axis-mismatch fixture, an extra source-axis brief key,
    // and a missing required brief.
    let v = load("target.schema.json");
    assert_invalid(&v, &yaml(TARGET_INVALID_AXIS_SOURCE), "target/axis-source");
    assert_invalid(
        &v,
        &yaml(TARGET_INVALID_BRIEFS_INCLUDE_EXTRACT),
        "target/briefs-include-extract",
    );
    assert_invalid(&v, &yaml(TARGET_INVALID_MISSING_MERGE_BRIEF), "target/missing-merge-brief");
}

// --- evidence.schema.json ------------------------------------------

const EVIDENCE_VALID_REQUIREMENT: &str = r"
source: legacy-monolith
adapter: code-typescript
authority: behaviour
lead: user-registration
claims:
  - kind: requirement
    claim-id: users.register.email-validation
    path: src/users/register.ts#L12-L87
    statement: The system accepts registrations with RFC 5322 emails.
";

const EVIDENCE_VALID_SPATIAL: &str = r"
source: home-screenshot
adapter: screenshots
authority: documentation
lead: home-screen
claims:
  - kind: region
    path: screenshots/home.png
  - kind: container
    path: screenshots/home.png
  - kind: leaf
    path: screenshots/home.png
";

const EVIDENCE_VALID_EMPTY_CLAIMS: &str = r"
source: intent
adapter: intent
authority: intent
lead: add-search-filter
claims: []
";

const EVIDENCE_INVALID_MISSING_AUTHORITY: &str = r"
source: legacy-monolith
adapter: code-typescript
lead: user-registration
claims: []
";

const EVIDENCE_INVALID_BAD_AUTHORITY: &str = r"
source: legacy-monolith
adapter: code-typescript
authority: unknown
lead: user-registration
claims: []
";

const EVIDENCE_INVALID_BAD_KIND: &str = r"
source: legacy-monolith
adapter: code-typescript
authority: behaviour
lead: user-registration
claims:
  - kind: hunch
    claim-id: users.register.maybe
";

const EVIDENCE_INVALID_REQUIREMENT_NO_CLAIM_ID: &str = r"
source: notes
adapter: documentation
authority: documentation
lead: password-reset
claims:
  - kind: requirement
    statement: Reset links expire after 30 minutes.
";

const EVIDENCE_INVALID_SOURCE_NOT_KEBAB: &str = r"
source: LegacyMonolith
adapter: code-typescript
authority: behaviour
lead: user-registration
claims: []
";

#[test]
fn evidence_accepts_doc_legacy_and_spatial() {
    let v = load("evidence.schema.json");
    assert_valid(&v, &yaml(EVIDENCE_VALID_REQUIREMENT), "evidence/requirement");
    assert_valid(&v, &yaml(EVIDENCE_VALID_SPATIAL), "evidence/spatial-region-container-leaf");
    assert_valid(&v, &yaml(EVIDENCE_VALID_EMPTY_CLAIMS), "evidence/empty-claims");
}

#[test]
fn evidence_rejects_bad_authority_and_kinds() {
    let v = load("evidence.schema.json");
    assert_invalid(&v, &yaml(EVIDENCE_INVALID_MISSING_AUTHORITY), "evidence/missing-authority");
    assert_invalid(&v, &yaml(EVIDENCE_INVALID_BAD_AUTHORITY), "evidence/bad-authority");
    assert_invalid(&v, &yaml(EVIDENCE_INVALID_BAD_KIND), "evidence/bad-kind");
    assert_invalid(
        &v,
        &yaml(EVIDENCE_INVALID_REQUIREMENT_NO_CLAIM_ID),
        "evidence/requirement-missing-claim-id",
    );
    assert_invalid(&v, &yaml(EVIDENCE_INVALID_SOURCE_NOT_KEBAB), "evidence/source-not-kebab");
}

// --- discovery/lead.schema.json --------------------------------

const LEAD_VALID: &str = r"
id: user-registration
sources: [legacy-monolith]
summary: Registration endpoint accepting email + password with RFC 5322 validation.
";

const LEAD_VALID_TENTATIVE: &str = r"
id: password-reset
sources: [identity-design-notes, legacy-monolith]
summary: Operator-initiated password reset via email link.
tentative: true
";

const LEAD_INVALID_NO_SOURCES: &str = r"
id: user-registration
sources: []
summary: bad — sources must be non-empty.
";

const LEAD_INVALID_BAD_ID: &str = r"
id: User_Registration
sources: [legacy-monolith]
summary: Bad id.
";

const LEAD_INVALID_TENTATIVE_WRONG_TYPE: &str = r"
id: user-registration
sources: [legacy-monolith]
summary: Bad tentative.
tentative: maybe
";

#[test]
fn lead_accepts_minimal_and_tentative_shapes() {
    let v = load("discovery/lead.schema.json");
    assert_valid(&v, &yaml(LEAD_VALID), "lead/minimal");
    assert_valid(&v, &yaml(LEAD_VALID_TENTATIVE), "lead/tentative");
}

#[test]
fn lead_rejects_bad_sources_id_and_tentative() {
    let v = load("discovery/lead.schema.json");
    assert_invalid(&v, &yaml(LEAD_INVALID_NO_SOURCES), "lead/no-sources");
    assert_invalid(&v, &yaml(LEAD_INVALID_BAD_ID), "lead/bad-id");
    assert_invalid(&v, &yaml(LEAD_INVALID_TENTATIVE_WRONG_TYPE), "lead/wrong-tentative-type");
}

// --- plan/plan.schema.json (source/target adapter split deltas) -------------------------

fn plan_v2_fixture_path(name: &str) -> PathBuf {
    // `crates/domain/tests/` -> `crates/domain/` -> `crates/` -> repo
    // root -> `tests/fixtures/plan/v2/`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures/plan/v2")
        .join(name)
}

#[test]
fn plan_schema_accepts_workflow_intent_n1() {
    let v = load("plan/plan.schema.json");
    let raw = std::fs::read_to_string(plan_v2_fixture_path("intent-n1.yaml")).expect("read");
    assert_valid(&v, &yaml(&raw), "plan/v2/intent-n1");
}

#[test]
fn plan_accepts_multi_source() {
    let v = load("plan/plan.schema.json");
    let raw = std::fs::read_to_string(plan_v2_fixture_path("multi-source.yaml")).expect("read");
    assert_valid(&v, &yaml(&raw), "plan/v2/multi-source");
}

#[test]
fn plan_accepts_divergence_likely() {
    let v = load("plan/plan.schema.json");
    let raw =
        std::fs::read_to_string(plan_v2_fixture_path("divergence-likely.yaml")).expect("read");
    assert_valid(&v, &yaml(&raw), "plan/v2/divergence-likely");
}

#[test]
fn plan_rejects_unknown_divergence() {
    let v = load("plan/plan.schema.json");
    let raw =
        std::fs::read_to_string(plan_v2_fixture_path("divergence-likely.yaml")).expect("read");
    let mutated = raw.replace("divergence: likely", "divergence: maybe");
    assert_invalid(&v, &yaml(&mutated), "plan/v2/divergence-bad-value");
}

#[test]
fn plan_rejects_slice_missing_lead() {
    let v = load("plan/plan.schema.json");
    let bad = r"
name: bad
slices:
  - name: only
    target: omnia@v1
    sources:
      - key: docs
    status: pending
";
    assert_invalid(&v, &yaml(bad), "plan/v2/source-missing-lead");
}
