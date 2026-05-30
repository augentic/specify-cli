//! Embedded JSON Schemas.
//!
//! Each constant is the verbatim contents of a file under
//! `specify-cli/schemas/` baked into the binary at compile time. See
//! `ResolvedRules` export contract and §"Structured lint finding
//! schema" for the standards-layer schemas; the workflow schemas are
//! pinned by the workflow contract under `docs/standards/workflow.md`.

/// Schema for `plan.yaml` (workflow contract — `slices[].sources[]`
/// bindings, `target`, slice-level `divergence` enum).
pub const PLAN_JSON_SCHEMA: &str = include_str!("../../../schemas/plan/plan.schema.json");

/// Schema for per-source `Evidence` files under
/// `.specify/slices/<name>/evidence/`.
pub const EVIDENCE_JSON_SCHEMA: &str = include_str!("../../../schemas/evidence.schema.json");

/// Schema for a single `Lead` block under `## Lead inventory` in
/// `discovery.md`. The `survey` operation validates each produced lead
/// against this shape before `Discovery::merge_survey` makes it visible.
pub const LEAD_JSON_SCHEMA: &str = include_str!("../../../schemas/discovery/lead.schema.json");

/// Schema for `provenance.yaml`, the audit-only provenance
/// index emitted by slice synthesis.
pub const PROVENANCE_JSON_SCHEMA: &str =
    include_str!("../../../schemas/slice/provenance.schema.json");

/// Schema for `components.yaml`, the operator-curated design-system
/// component catalog.
pub const COMPONENTS_JSON_SCHEMA: &str =
    include_str!("../../../schemas/design-system/components.schema.json");

/// Schema for the `specrun rules export` payload — the resolved rules
/// tree consumed by lint tooling. See `ResolvedRules` export contract.
pub const RESOLVED_RULES_JSON_SCHEMA: &str =
    include_str!("../../../schemas/rules/resolved.schema.json");

/// Schema for a single rule frontmatter block. See the rule file shape contract.
pub const RULE_JSON_SCHEMA: &str = include_str!("../../../schemas/rules/rule.schema.json");

/// Schema for the neutral `Diagnostic` wire shape produced by every
/// check surface (lint and validate alike) and validated at the
/// diagnostic boundary. See the structured diagnostic schema.
pub const DIAGNOSTIC_JSON_SCHEMA: &str =
    include_str!("../../../schemas/diagnostics/diagnostic.schema.json");

/// Schema for the v1 `WorkspaceModel` envelope produced once per
/// `specrun lint` invocation.
///
/// See the `WorkspaceModel` schema and schema-location contract; the
/// `version: 1` discriminant pins the v1 shape defined here.
pub const WORKSPACE_MODEL_JSON_SCHEMA: &str =
    include_str!("../../../schemas/lint/workspace-model.schema.json");

/// Schema for the `DiagnosticReport` envelope (`{ version, summary, findings }`).
///
/// Validated before stdout emit by every diagnostic surface
/// (`specrun lint --format json` and the workflow-gating validate
/// surface alike). The `findings[]` element shape lives in
/// [`DIAGNOSTIC_JSON_SCHEMA`] and is wired via a relative
/// `diagnostic.schema.json` `$ref`.
pub const DIAGNOSTIC_REPORT_JSON_SCHEMA: &str =
    include_str!("../../../schemas/diagnostics/diagnostic-report.schema.json");

/// Schema for `SKILL.md` YAML frontmatter (framework authoring).
pub const SKILL_JSON_SCHEMA: &str = include_str!("../../../schemas/authoring/skill.schema.json");

/// Schema for scenario-pack YAML frontmatter (framework authoring).
pub const SCENARIO_JSON_SCHEMA: &str =
    include_str!("../../../schemas/authoring/scenario.schema.json");

/// Schema for `.cursor-plugin/marketplace.json` (framework authoring).
pub const MARKETPLACE_JSON_SCHEMA: &str =
    include_str!("../../../schemas/authoring/marketplace.schema.json");
