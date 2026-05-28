//! Embedded JSON Schemas.
//!
//! Each constant is the verbatim contents of a file under
//! `specify-cli/schemas/` baked into the binary at compile time. See
//! RFC-28 §"Resolved codex export" and §"Structured lint finding
//! schema" for the standards-layer schemas; the workflow schemas are
//! pinned by the workflow contract under `docs/standards/workflow.md`.

/// Schema for `plan.yaml` (workflow contract — `slices[].sources[]`
/// bindings, `target`, slice-level `divergence` enum).
pub const PLAN_JSON_SCHEMA: &str = include_str!("../../../schemas/plan/plan.schema.json");

/// Schema for per-source `Evidence` files under
/// `.specify/slices/<name>/evidence/`.
pub const EVIDENCE_JSON_SCHEMA: &str = include_str!("../../../schemas/evidence.schema.json");

/// Schema for `fusion.yaml`, the audit-only reconciliation index
/// emitted by slice synthesis.
pub const FUSION_JSON_SCHEMA: &str = include_str!("../../../schemas/slice/fusion.schema.json");

/// Schema for `components.yaml`, the operator-curated design-system
/// component catalog.
pub const COMPONENTS_JSON_SCHEMA: &str =
    include_str!("../../../schemas/design-system/components.schema.json");

/// Schema for the `specrun rules export` payload — the resolved codex
/// tree consumed by lint tooling. See RFC-28 §"Resolved codex export".
pub const RESOLVED_CODEX_JSON_SCHEMA: &str =
    include_str!("../../../schemas/codex/resolved.schema.json");

/// Schema for a single codex-rule frontmatter block. See RFC-28
/// §"Codex file shape".
pub const CODEX_RULE_JSON_SCHEMA: &str =
    include_str!("../../../schemas/codex/codex-rule.schema.json");

/// Schema for the `LintFinding` wire shape produced by lint tooling
/// and validated at the finding boundary. See RFC-28 §"Structured
/// lint finding schema".
pub const LINT_FINDING_JSON_SCHEMA: &str =
    include_str!("../../../schemas/lint/finding.schema.json");

/// Schema for the v1 `WorkspaceModel` envelope produced once per
/// `specrun lint` invocation.
///
/// See RFC-32 §"`WorkspaceModel`" and §"Schema location"; the
/// `version: 1` discriminant pins the v1 shape defined here.
pub const WORKSPACE_MODEL_JSON_SCHEMA: &str =
    include_str!("../../../schemas/lint/workspace-model.schema.json");

/// Schema for the `specrun lint --format json` envelope
/// (`{ version, summary, findings }`) validated before stdout emit.
///
/// The `findings[]` element shape lives in [`LINT_FINDING_JSON_SCHEMA`]
/// and is wired via a relative `finding.schema.json` `$ref`. See RFC-32
/// §D9.
pub const LINT_RESULT_JSON_SCHEMA: &str =
    include_str!("../../../schemas/lint/lint-result.schema.json");
