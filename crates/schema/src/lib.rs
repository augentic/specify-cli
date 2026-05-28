//! Embedded JSON Schemas and the JSON-Schema validation plumbing
//! shared between `specify-domain` (workflow artifacts) and the future
//! `specify-lints` standards-layer crate, per [DECISIONS.md § Standards layer split into `specify-lints` and `specify-schema`](../../DECISIONS.md#standards-layer-split-into-specify-lints-and-specify-schema).
//!
//! Schemas are bundled at compile time via `include_str!` so the binary
//! carries them with no runtime filesystem lookup. The helpers in
//! [`validate`] convert `jsonschema` validator output into the shared
//! [`specify_error::ValidationSummary`] shape that callers fold into an
//! [`specify_error::Error::Validation`] (exit code 2) or
//! [`specify_error::Error::Diag`] (exit code 1) as their policy
//! dictates.

pub mod constants;
pub mod validate;

pub use constants::{
    COMPONENTS_JSON_SCHEMA, EVIDENCE_JSON_SCHEMA, FUSION_JSON_SCHEMA, LINT_FINDING_JSON_SCHEMA,
    LINT_RESULT_JSON_SCHEMA, PLAN_JSON_SCHEMA, RESOLVED_RULES_JSON_SCHEMA, RULE_JSON_SCHEMA,
    WORKSPACE_MODEL_JSON_SCHEMA,
};
pub use validate::{compile_schema, read_yaml_as_json, validate_serialisable, validate_value};
