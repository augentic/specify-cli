//! Codex DTOs and runtime wire types per the rules contract.
//!
//! Provides the typed Rust shapes that round-trip cleanly through the
//! schemas embedded under `schemas/rules/` and `schemas/lint/`:
//!
//! - [`Rule`] / [`Deprecated`] / [`Applicability`] /
//!   [`DeterministicHint`] / [`Reference`] are the parsed-frontmatter
//!   shape used by the CH-11 frontmatter parser. Field names are
//!   kebab-case at every nesting level (`lint-mode`,
//!   `deterministic-hints`, `replaced-by`); the parser performs the
//!   `snake_case -> kebab-case` lift on the raw markdown side so the
//!   in-memory shape matches the wire shape.
//! - [`ResolvedRules`] / [`ResolvedRule`] are the export envelope
//!   emitted by `specrun rules export --format json` (CH-17). They
//!   add resolver-only fields ([`Origin`], [`PathRoot`], `path`,
//!   `body`) on top of the codex-rule shape.
//! - [`LintFinding`] / [`FindingEvidence`] / [`FindingLocation`] /
//!   [`FindingSource`] / [`Artifact`] / [`Confidence`] /
//!   [`FindingStatus`] / [`FindingDisposition`] /
//!   [`DirectiveDisposition`] / [`DispositionSource`] are the
//!   structured review-finding shape shared by `specrun lint`, target
//!   adapter review briefs, and CI annotations (CH-16/CH-21).
//!
//! Severity comparator order is `Critical < Important < Suggestion <
//! Optional` and origin order is `Target < Source < Shared <
//! Unknown`, matching `ResolvedRules` export contract
//! §"Ordering". The closed enums are declared in the comparator order
//! so the derived [`Ord`] picks up the contract-defined sort sequence.

#![allow(
    clippy::module_name_repetitions,
    reason = "The public wire contract uses the names Rule and ResolvedRules; renaming to avoid the codex prefix would obscure the schema mapping."
)]

pub mod finding;
pub mod fingerprint;
pub mod parse;
pub mod resolve;

pub use finding::{
    FindingError, validate, validate_evidence_size, validate_finding, validate_finding_json,
    validate_fingerprint,
};
pub use parse::{ParseError, parse_rule, parse_rule_file};
pub use resolve::{
    ResolveError, ResolveInputs, ResolvedRuleEntry, build_resolved_rules, filter, resolve,
    sort_resolved,
};
use serde::{Deserialize, Serialize};

/// Closed severity enum per `ResolvedRules` export contract. Variants
/// are declared in the documented sort order — the derived [`Ord`]
/// therefore yields `Critical < Important < Suggestion < Optional`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Highest priority; blocks merge in CI.
    Critical,
    /// Should-fix; default escalation level for adapter overlays.
    Important,
    /// Nice-to-have; reviewer judgement applies.
    Suggestion,
    /// Informational; recorded but not graded.
    Optional,
}

/// Resolver origin tier per `ResolvedRules` export contract.
///
/// Variants are declared in the documented sort order (`target`,
/// `source`, `shared`, `core`, `unknown`) so the derived [`Ord`]
/// yields the contract-defined comparator. Wire spelling uses
/// kebab-case rendered from the variant identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Target-adapter overlay (`adapters/targets/<name>/rules/`).
    Target,
    /// Source-adapter overlay (`adapters/sources/<name>/rules/`).
    Source,
    /// Shared rules (`adapters/shared/rules/...`).
    Shared,
    /// Core pack overlay. Variant and sort slot only — the producing
    /// path is wired in a follow-up card; no resolver/indexer site
    /// emits this value yet.
    Core,
    /// Indexer fallback: cache rule files whose path does not match
    /// the closed adapter-shape probe in `infer_origin` under
    /// [`crate::lint::index`].
    Unknown,
}

/// Anchor for the rule `path` field in a [`ResolvedRule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathRoot {
    /// Path is relative to the rules root (shared rules and rules-root
    /// fallback overlays).
    RulesRoot,
    /// Path is relative to the project directory (project-local and
    /// cached overlays).
    ProjectDir,
}

/// How a rule is expected to be reviewed. Wire spelling is
/// kebab-case (`deterministic`, `model-assisted`, `hybrid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LintMode {
    /// Rule is fully expressed as deterministic hints.
    Deterministic,
    /// Rule needs an SLM/LLM scorer.
    ModelAssisted,
    /// Mix of deterministic + model-assisted signals.
    Hybrid,
}

/// Closed v1 deterministic-hint kind enum.
///
/// Includes the executable v1 kinds (`path-pattern`, `regex`,
/// `schema`, `tool`, `reference-resolves`, `unique`,
/// `set-coverage`, `cardinality`, `constant-eq`) and the Reserved
/// hint kind kinds (`set-eq`, `content-digest-eq`, `namespace-owner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HintKind {
    /// Glob pattern matched against artifact paths.
    PathPattern,
    /// Regular expression matched against artifact bytes.
    Regex,
    /// Validate against an embedded JSON Schema.
    Schema,
    /// Invoke a declared WASI tool.
    Tool,
    /// Assert that some field across a candidate set is unique
    /// (v1 source discriminator: `skill-name`).
    Unique,
    /// Every reference resolves (v1 source discriminator: `markdown-link`).
    ReferenceResolves,
    /// Assert that the values some candidate file declares cover a
    /// closed expected set (v1 source discriminator:
    /// `adapter-briefs-cover-operations`).
    SetCoverage,
    /// Assert that some countable property of a candidate is within
    /// configured bounds (v1 source discriminator:
    /// `skill-body-line-count-max-200`).
    Cardinality,
    /// Assert that an extracted field on a candidate fact equals a
    /// configured constant (v1 source discriminator:
    /// `adapter-manifest-version-equals-v1`).
    ConstantEq,
    /// Reserved hint kind: assert two sets are equal.
    SetEq,
    /// Reserved hint kind: assert two content digests are equal.
    ContentDigestEq,
    /// Reserved hint kind: assert a namespace owner.
    NamespaceOwner,
}

/// Producer attribution for a [`LintFinding`]. Distinct from the
/// codex-rule [`LintMode`] enum because review findings may also
/// originate from a human reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingSource {
    /// Output of a deterministic scanner.
    Deterministic,
    /// Output of an SLM/LLM scorer.
    ModelAssisted,
    /// Mix of deterministic + model-assisted signals.
    Hybrid,
    /// Recorded by a human reviewer.
    Human,
}

/// Artifact category attribution for a [`LintFinding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Artifact {
    /// Generated or hand-written code.
    Code,
    /// Test files.
    Tests,
    /// Contract artifacts under `contracts/`.
    Contracts,
    /// Behavioral specs (`spec.md`).
    Specs,
    /// Design notes (`design.md`).
    Design,
    /// Task list (`tasks.md`).
    Tasks,
    /// Asset inventory (`assets.yaml`).
    Assets,
    /// Design tokens (`tokens.yaml`).
    Tokens,
    /// Per-shell composition manifest.
    Composition,
    /// Artifact category not classified.
    Unknown,
}

/// Producer self-rated confidence for a [`LintFinding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    /// High confidence in the finding.
    High,
    /// Medium confidence.
    Medium,
    /// Low confidence; reviewer should triage.
    Low,
}

/// Triage status for a [`LintFinding`]. Omitted by raw scanners and
/// populated by review reports, the RFC-33a directive post-pass, or
/// CI state.
///
/// `Ignored` is set by the directive pass when a `specify-ignore`
/// directive matches a finding; `FalsePositive` is set by the same
/// pass when the directive's rationale begins with `false-positive:`.
/// Wire spelling stays kebab-case (`ignored`, `false-positive`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingStatus {
    /// Untriaged; default for fresh findings and the only
    /// default-blocking value at exit time.
    Open,
    /// Demoted by a matching `specify-ignore` directive (RFC-33a).
    Ignored,
    /// Resolved by a code change.
    Fixed,
    /// Operator-acknowledged; will not be fixed.
    Accepted,
    /// Producer-mistaken; the finding does not apply.
    FalsePositive,
}

/// Origin of a non-`open` `status` on a [`LintFinding`].
///
/// Closed discriminator for the `disposition.source` wire field.
/// RFC-33a producers emit `Directive`; RFC-33b will additively add
/// `Baseline` when it lands. Wire spelling is kebab-case
/// (`directive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DispositionSource {
    /// `specify-ignore` directive in the scanned source.
    Directive,
}

/// `disposition.directive` payload populated when
/// [`FindingDisposition::source`] is [`DispositionSource::Directive`].
///
/// Carries the verbatim directive site (`path`, `line`) and the
/// free-form `rationale` captured from the comment. The
/// directive-validation pass surfaces short rationales as `UNI-022`
/// findings; the rationale string itself is still captured here so
/// downstream tooling can render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DirectiveDisposition {
    /// Project-relative path of the source file containing the
    /// directive comment.
    pub path: String,
    /// 1-based line of the directive comment itself (not the target
    /// line the directive applies to).
    pub line: u32,
    /// Free-form rationale captured verbatim from the directive
    /// comment.
    pub rationale: String,
}

/// Origin of a non-`open` finding status on a [`LintFinding`].
///
/// Unset when `status` is `open` or absent; required to carry
/// `source` when present. RFC-33a producers populate `directive`;
/// RFC-33b will additively populate a `baseline` sub-field when it
/// lands. `since` is reserved for downstream tooling and left unset
/// by RFC-33a emitters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FindingDisposition {
    /// Closed discriminator naming the disposition's origin.
    pub source: DispositionSource,
    /// Directive payload, populated when `source` is
    /// [`DispositionSource::Directive`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directive: Option<DirectiveDisposition>,
    /// Optional free-form marker indicating when the disposition took
    /// effect (commit hash, ISO-8601 timestamp, release tag, …).
    /// RFC-33a emitters leave this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

/// Inclusive narrowing filter — all populated dimensions match (AND).
///
/// Per the wire schema at least one dimension must be populated; the
/// Rust shape represents every dimension as `Option` and the
/// resolver enforces the at-least-one rule when it loads the rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Applicability {
    /// Adapter names this rule applies to (with optional `@v<major>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapters: Option<Vec<String>>,
    /// Language tokens this rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<String>>,
    /// Artifact categories this rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<String>>,
    /// Project-relative path globs this rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

/// One deterministic-hint entry on a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DeterministicHint {
    /// Hint kind discriminator.
    pub kind: HintKind,
    /// Hint payload, interpreted by a future validator or review tool.
    pub value: String,
    /// Optional human explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One reference entry on a rule. Schema requires `label` plus
/// at least one of `url` / `path`; the resolver enforces the
/// `anyOf` rule when it loads the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Reference {
    /// Short display label.
    pub label: String,
    /// HTTP(S) reference URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Repository-relative reference path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Deprecation metadata for a rule.
///
/// Marks a rule as deprecated while preserving the stable id for
/// historical citations. Wire key for `replaced_by` is the
/// kebab-case `replaced-by` per `ResolvedRules` export contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Deprecated {
    /// Human-readable reason the rule is deprecated.
    pub reason: String,
    /// Replacement rule id when there is a direct successor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
}

/// Parsed rule (frontmatter + body) on the wire-kebab shape.
///
/// The CH-11 frontmatter parser owns the snake-to-kebab lift on the
/// markdown authoring side; this struct represents the post-lift
/// shape so it round-trips cleanly through the resolved-export
/// schema (the resolver-only fields — `origin`, `path-root`, `path`
/// — live on [`ResolvedRule`], not here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Rule {
    /// Stable rule identifier (e.g. `UNI-014`).
    pub id: String,
    /// Short human-readable rule title.
    pub title: String,
    /// Default review severity for findings citing this rule.
    pub severity: Severity,
    /// One-sentence trigger condition.
    pub trigger: String,
    /// How the rule is expected to be reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lint_mode: Option<LintMode>,
    /// Inclusive narrowing filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicability: Option<Applicability>,
    /// Optional deterministic-hint list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deterministic_hints: Option<Vec<DeterministicHint>>,
    /// Optional reference list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<Reference>>,
    /// Optional deprecation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<Deprecated>,
    /// Verbatim markdown body (everything after the closing
    /// frontmatter delimiter), including section headings such as
    /// `## Rule`.
    #[serde(default)]
    pub body: String,
}

/// Read-only resolved view of shared, source-adapter, and
/// target-adapter rules. Wire envelope emitted by
/// `specrun rules export --format json` per the rules contract
/// §"Resolved rules export".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ResolvedRules {
    /// Envelope version. v1 pins this to 1.
    pub version: u32,
    /// Resolved target-adapter name, optionally `@v<major>`.
    pub target_adapter: String,
    /// Source-adapter names bound to the export context.
    pub source_adapters: Vec<String>,
    /// Ordered rule list, `(non-deprecated, severity, origin,
    /// rule-id)` per `ResolvedRules` export contract §"Ordering".
    pub rules: Vec<ResolvedRule>,
}

/// One resolved rule entry inside a [`ResolvedRules`]. Carries every
/// codex-rule field plus resolver-only fields (`origin`,
/// `path-root`, `path`, `body`).
///
/// The `rule_id` field is named distinctly from [`Rule::id`] so
/// the wire shape stabilises on `rule-id` per the wire contract; semantically
/// the value is the same identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ResolvedRule {
    /// Rule id (e.g. `UNI-014`).
    pub rule_id: String,
    /// Short human-readable rule title.
    pub title: String,
    /// Default review severity for findings citing this rule.
    pub severity: Severity,
    /// One-sentence trigger condition.
    pub trigger: String,
    /// How the rule is expected to be reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lint_mode: Option<LintMode>,
    /// Inclusive narrowing filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicability: Option<Applicability>,
    /// Optional deterministic-hint list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deterministic_hints: Option<Vec<DeterministicHint>>,
    /// Optional reference list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<Reference>>,
    /// Resolver origin tier.
    pub origin: Origin,
    /// Anchor for the rule `path` field.
    pub path_root: PathRoot,
    /// Path to the rule markdown file, relative to `path-root`.
    pub path: String,
    /// Verbatim markdown body (everything after the closing `---`
    /// frontmatter delimiter), including section headings such as
    /// `## Rule`.
    pub body: String,
    /// Deprecation metadata, or `null` when the rule is active.
    #[serde(default)]
    pub deprecated: Option<Deprecated>,
}

/// File path plus optional line/column range carried by a
/// [`LintFinding`] or by a `digest`/`structured` evidence variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FindingLocation {
    /// Project-relative file path.
    pub path: String,
    /// Anchor line (0-indexed; producers commonly emit 1-indexed and
    /// the schema accepts either with `minimum: 0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Anchor column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Inclusive end line for a multi-line range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// Inclusive end column for a multi-line range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
}

/// Closed evidence union for a [`LintFinding`].
///
/// Internally tagged on `kind`; the wire shape's `oneOf` is encoded
/// by serde's `tag = "kind"` with `additionalProperties: false` per
/// branch validated schema-side.
///
/// Bounded verbatim payloads use [`FindingEvidence::Snippet`];
/// payloads too large or sensitive to inline use
/// [`FindingEvidence::Digest`]; domain-structured payloads (e.g.
/// contract compatibility metadata) use
/// [`FindingEvidence::Structured`]. The lint finding contract caps the serialized
/// evidence payload at 16 `KiB`; that ceiling is enforced by the
/// CH-16 finding validator, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FindingEvidence {
    /// Bounded verbatim excerpt for local code or prose evidence.
    Snippet {
        /// Verbatim payload bytes.
        value: String,
    },
    /// Digest reference for evidence too large or sensitive to inline.
    Digest {
        /// Hex-encoded SHA-256 of the underlying evidence bytes.
        sha256: String,
        /// Short human summary of what was hashed.
        summary: String,
        /// Optional contributing locations referenced by the digest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<FindingLocation>>,
    },
    /// Domain-structured evidence (e.g. contract compatibility data).
    Structured {
        /// Short human summary of `data`.
        summary: String,
        /// Free-form JSON payload. Producers MUST keep `data` bounded
        /// and secret-free; the CH-16 validator enforces the 16 `KiB`
        /// cap on the full evidence object.
        data: serde_json::Value,
        /// Optional contributing locations referenced by the payload.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<FindingLocation>>,
    },
}

/// Structured review finding per the rules contract.
///
/// Shared by deterministic scanners (`specrun lint`), framework
/// JSON export (`specdev lint --format json`), target adapter
/// review briefs, model-assisted scorers, CI annotations, and
/// dashboards.
///
/// Producer-local `id` (e.g. `FIND-0001`) is distinct from the codex
/// `rule_id` (e.g. `UNI-014`): `id` is a stable per-run handle and
/// `rule_id` is the durable codex citation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LintFinding {
    /// Producer-local stable id for this run (e.g. `FIND-0001`).
    pub id: String,
    /// Rule id (e.g. `UNI-014`); absent for findings that do
    /// not cite codex policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    /// Additional codex ids that informed the finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_rule_ids: Option<Vec<String>>,
    /// Short finding title.
    pub title: String,
    /// Closed severity enum; uses the same values as [`Severity`].
    pub severity: Severity,
    /// Producer attribution.
    pub source: FindingSource,
    /// Target-adapter name when the finding is adapter-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_adapter: Option<String>,
    /// Source-adapter name when the finding is source-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_adapter: Option<String>,
    /// Slice name when the finding is slice-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    /// Change name when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// Artifact category attribution.
    pub artifact: Artifact,
    /// Optional anchor location for the finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<FindingLocation>,
    /// Evidence union per the structured evidence union.
    pub evidence: FindingEvidence,
    /// Operator-facing risk.
    pub impact: String,
    /// Concrete action to clear the finding.
    pub remediation: String,
    /// Producer self-rated confidence. Required for
    /// `source: model-assisted`; the conditional rule is enforced by
    /// the CH-16 validator, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// Stable hash over `(rule-id, location, evidence-payload)` per
    /// the structured lint finding schema §"Fingerprint
    /// algorithm". Format `sha256:<64 hex chars>`.
    pub fingerprint: String,
    /// Triage status. Omitted by raw scanners; populated by review
    /// reports, the RFC-33a directive post-pass, or CI state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<FindingStatus>,
    /// Origin of a non-`open` `status`. Unset when `status` is
    /// `open` or absent. RFC-33a producers populate
    /// `disposition.directive`; the field is excluded from the
    /// fingerprint per the schema's fingerprint contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<FindingDisposition>,
}

#[cfg(test)]
mod tests {
    use jsonschema::Validator;
    use serde_json::{Value as JsonValue, json};
    use specify_schema::{LINT_FINDING_JSON_SCHEMA, RESOLVED_RULES_JSON_SCHEMA, RULE_JSON_SCHEMA};

    use super::{
        Applicability, Artifact, Confidence, Deprecated, DeterministicHint, DirectiveDisposition,
        DispositionSource, FindingDisposition, FindingEvidence, FindingLocation, FindingSource,
        FindingStatus, HintKind, LintFinding, LintMode, Origin, PathRoot, Reference, ResolvedRule,
        ResolvedRules, Rule, Severity,
    };

    fn validator(schema_source: &str) -> Validator {
        let schema: JsonValue = serde_json::from_str(schema_source).expect("schema parses");
        jsonschema::validator_for(&schema).expect("schema compiles")
    }

    fn assert_validates(schema_source: &str, instance: &JsonValue) {
        let v = validator(schema_source);
        let errors: Vec<String> = v.iter_errors(instance).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "instance must validate; errors: {errors:?}");
    }

    /// `ResolvedRules` export contract: severity comparator order is
    /// `critical < important < suggestion < optional`.
    #[test]
    fn severity_ordering_matches_contract() {
        assert!(Severity::Critical < Severity::Important);
        assert!(Severity::Important < Severity::Suggestion);
        assert!(Severity::Suggestion < Severity::Optional);
        let mut shuffled =
            vec![Severity::Optional, Severity::Critical, Severity::Suggestion, Severity::Important];
        shuffled.sort();
        assert_eq!(
            shuffled,
            vec![Severity::Critical, Severity::Important, Severity::Suggestion, Severity::Optional,]
        );
    }

    /// `ResolvedRules` export contract §"Ordering": origin comparator
    /// order is `target, source, shared, core, unknown`.
    #[test]
    fn origin_ordering_matches_contract() {
        assert!(Origin::Target < Origin::Source);
        assert!(Origin::Source < Origin::Shared);
        assert!(Origin::Shared < Origin::Core);
        assert!(Origin::Core < Origin::Unknown);
        let mut shuffled =
            vec![Origin::Unknown, Origin::Core, Origin::Shared, Origin::Target, Origin::Source];
        shuffled.sort();
        assert_eq!(
            shuffled,
            vec![Origin::Target, Origin::Source, Origin::Shared, Origin::Core, Origin::Unknown]
        );
    }

    /// `Rule` round-trips its own JSON shape, exercising the
    /// snake-to-kebab field renames (`lint-mode`,
    /// `deterministic-hints`).
    #[test]
    fn codex_rule_round_trips() {
        let rule = Rule {
            id: "UNI-014".into(),
            title: "Hardcoded Configuration".into(),
            severity: Severity::Important,
            trigger: "Generated code embeds environment-specific configuration instead of routing it through declared configuration.".into(),
            lint_mode: Some(LintMode::Hybrid),
            applicability: Some(Applicability {
                adapters: Some(vec!["omnia".into()]),
                languages: Some(vec!["rust".into()]),
                artifacts: Some(vec!["code".into()]),
                paths: None,
            }),
            deterministic_hints: Some(vec![DeterministicHint {
                kind: HintKind::Regex,
                value: "https?://".into(),
                description: Some("Literal URL in generated code.".into()),
            }]),
            references: Some(vec![Reference {
                label: "Omnia guardrails".into(),
                url: None,
                path: Some("adapters/targets/omnia/references/guardrails.md".into()),
            }]),
            deprecated: None,
            body: "## Rule\n\nConfiguration values that vary between deployments must not be hardcoded in generated code.\n".into(),
        };
        let value = serde_json::to_value(&rule).expect("serialise");
        assert_eq!(value.get("lint-mode").and_then(JsonValue::as_str), Some("hybrid"));
        assert!(value.get("deterministic-hints").is_some());
        let parsed: Rule = serde_json::from_value(value).expect("round-trip");
        assert_eq!(rule, parsed);
    }

    /// UNI-014 example builds from typed structs, validates
    /// against `resolved.schema.json`, and round-trips back to the
    /// same struct.
    #[test]
    fn resolved_codex_round_trips_against_schema() {
        let resolved = ResolvedRules {
            version: 1,
            target_adapter: "omnia".into(),
            source_adapters: vec!["code-typescript".into()],
            rules: vec![ResolvedRule {
                rule_id: "UNI-014".into(),
                title: "Hardcoded Configuration".into(),
                severity: Severity::Important,
                trigger: "Generated code embeds environment-specific configuration instead of routing it through declared configuration.".into(),
                lint_mode: Some(LintMode::Hybrid),
                applicability: Some(Applicability {
                    adapters: Some(vec!["omnia".into()]),
                    languages: Some(vec!["rust".into()]),
                    artifacts: Some(vec!["code".into()]),
                    paths: None,
                }),
                deterministic_hints: Some(vec![DeterministicHint {
                    kind: HintKind::Regex,
                    value: "https?://".into(),
                    description: Some("Literal URL in generated code.".into()),
                }]),
                references: Some(vec![Reference {
                    label: "Omnia guardrails".into(),
                    url: None,
                    path: Some("adapters/targets/omnia/references/guardrails.md".into()),
                }]),
                origin: Origin::Shared,
                path_root: PathRoot::RulesRoot,
                path: "adapters/shared/rules/universal/hardcoded-configuration.md".into(),
                body: "## Rule\n\nConfiguration values that vary between deployments must not be hardcoded in generated code.\n".into(),
                deprecated: None,
            }],
        };
        let value = serde_json::to_value(&resolved).expect("serialise");
        assert_validates(RESOLVED_RULES_JSON_SCHEMA, &value);
        let parsed: ResolvedRules = serde_json::from_value(value).expect("round-trip");
        assert_eq!(resolved, parsed);
    }

    /// `Deprecated.replaced_by` MUST serialise to the kebab-case wire
    /// key `replaced-by` per `ResolvedRules` export contract. Test
    /// covers the explicitly-called-out rename.
    #[test]
    fn deprecated_replaced_by_uses_kebab_wire_key() {
        let deprecated = Deprecated {
            reason: "superseded by SEC-001".into(),
            replaced_by: Some("SEC-001".into()),
        };
        let value = serde_json::to_value(&deprecated).expect("serialise");
        assert_eq!(value.get("replaced-by").and_then(JsonValue::as_str), Some("SEC-001"));
        assert!(value.get("replaced_by").is_none(), "snake_case wire key must not appear");

        let body = serde_json::to_string(&deprecated).expect("serialise");
        assert!(body.contains("\"replaced-by\""), "body must carry replaced-by; got {body}");
        assert!(!body.contains("replaced_by"), "snake_case must not leak; got {body}");

        let parsed: Deprecated = serde_json::from_value(value).expect("round-trip");
        assert_eq!(deprecated, parsed);

        // Sanity: the standalone `Rule` schema also reads the
        // post-lift kebab-case shape since CH-10 owns the wire-side
        // structs (the snake-cased authoring schema is exercised by
        // the parallel test in `schema.rs`).
        validator(RULE_JSON_SCHEMA);
    }

    /// FIND-0001 example builds from typed structs, validates
    /// against `finding.schema.json`, and round-trips back to the
    /// same struct.
    #[test]
    fn review_finding_round_trips_against_schema() {
        let finding = LintFinding {
            id: "FIND-0001".into(),
            rule_id: Some("UNI-014".into()),
            related_rule_ids: None,
            title: "Literal deployment URL in generated handler".into(),
            severity: Severity::Important,
            source: FindingSource::Hybrid,
            target_adapter: Some("omnia".into()),
            source_adapter: None,
            slice: Some("billing-invoice-export".into()),
            change: None,
            artifact: Artifact::Code,
            location: Some(FindingLocation {
                path: "crates/invoice_export/src/config.rs".into(),
                line: Some(18),
                column: None,
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Snippet {
                value: "const BASE_URL: &str = \"https://api.example.com\";".into(),
            },
            impact: "Generated code will point every deployment at the same external endpoint."
                .into(),
            remediation:
                "Read the endpoint from Omnia configuration and add a required config key to the design."
                    .into(),
            confidence: Some(Confidence::High),
            fingerprint:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
            status: None,
            disposition: None,
        };
        let value = serde_json::to_value(&finding).expect("serialise");
        assert_validates(LINT_FINDING_JSON_SCHEMA, &value);
        let parsed: LintFinding = serde_json::from_value(value).expect("round-trip");
        assert_eq!(finding, parsed);
    }

    /// Each [`FindingEvidence`] variant round-trips through the
    /// finding schema with its required fields populated.
    #[test]
    fn evidence_union_round_trips_each_variant() {
        let snippet = FindingEvidence::Snippet {
            value: "let x = 1;".into(),
        };
        let snippet_json = serde_json::to_value(&snippet).expect("serialise");
        assert_eq!(snippet_json["kind"], "snippet");
        let parsed: FindingEvidence =
            serde_json::from_value(snippet_json).expect("round-trip snippet");
        assert_eq!(snippet, parsed);

        let digest = FindingEvidence::Digest {
            sha256: "abcd".repeat(16),
            summary: "binary blob".into(),
            locations: Some(vec![FindingLocation {
                path: "src/lib.rs".into(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            }]),
        };
        let digest_json = serde_json::to_value(&digest).expect("serialise");
        assert_eq!(digest_json["kind"], "digest");
        let parsed: FindingEvidence =
            serde_json::from_value(digest_json).expect("round-trip digest");
        assert_eq!(digest, parsed);

        let structured = FindingEvidence::Structured {
            summary: "contract compat".into(),
            data: json!({"breaking": true, "removed": ["GET /v1/foo"]}),
            locations: None,
        };
        let structured_json = serde_json::to_value(&structured).expect("serialise");
        assert_eq!(structured_json["kind"], "structured");
        let parsed: FindingEvidence =
            serde_json::from_value(structured_json).expect("round-trip structured");
        assert_eq!(structured, parsed);

        // Each variant is a legal evidence payload inside a finding.
        for evidence in [snippet, digest, structured] {
            let finding = LintFinding {
                id: "FIND-0001".into(),
                rule_id: Some("UNI-014".into()),
                related_rule_ids: None,
                title: "evidence-union smoke".into(),
                severity: Severity::Suggestion,
                source: FindingSource::Deterministic,
                target_adapter: None,
                source_adapter: None,
                slice: None,
                change: None,
                artifact: Artifact::Code,
                location: None,
                evidence,
                impact: "n/a".into(),
                remediation: "n/a".into(),
                confidence: None,
                fingerprint:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
                status: Some(FindingStatus::Open),
                disposition: None,
            };
            let value = serde_json::to_value(&finding).expect("serialise");
            assert_validates(LINT_FINDING_JSON_SCHEMA, &value);
            let parsed: LintFinding =
                serde_json::from_value(value).expect("round-trip evidence variant");
            assert_eq!(finding, parsed);
        }
    }

    /// Template for the RFC-33a disposition round-trips below.
    fn disposition_fixture(
        status: FindingStatus, disposition: Option<FindingDisposition>,
    ) -> LintFinding {
        LintFinding {
            id: "FIND-0007".into(),
            rule_id: Some("UNI-014".into()),
            related_rule_ids: None,
            title: "Literal deployment URL in generated handler".into(),
            severity: Severity::Important,
            source: FindingSource::Deterministic,
            target_adapter: Some("omnia".into()),
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Code,
            location: Some(FindingLocation {
                path: "crates/invoice_export/src/config.rs".into(),
                line: Some(18),
                column: None,
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Snippet {
                value: "const BASE_URL: &str = \"https://api.example.com\";".into(),
            },
            impact: "Generated code points every deployment at one endpoint.".into(),
            remediation: "Route the endpoint through Omnia configuration.".into(),
            confidence: Some(Confidence::High),
            fingerprint: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            status: Some(status),
            disposition,
        }
    }

    /// RFC-33a D5 + D6: a finding stamped `status: ignored` plus a
    /// populated `disposition.directive` round-trips through the
    /// canonical JSON shape, validates against the embedded schema,
    /// and deserialises back to an identical struct.
    #[test]
    fn ignored_directive_round_trips() {
        let finding = disposition_fixture(
            FindingStatus::Ignored,
            Some(FindingDisposition {
                source: DispositionSource::Directive,
                directive: Some(DirectiveDisposition {
                    path: "crates/invoice_export/src/config.rs".into(),
                    line: 17,
                    rationale: "internal deploy only — endpoint pinned per ops policy".into(),
                }),
                since: None,
            }),
        );
        let value = serde_json::to_value(&finding).expect("serialise");
        assert_validates(LINT_FINDING_JSON_SCHEMA, &value);
        assert_eq!(value.get("status").and_then(JsonValue::as_str), Some("ignored"));
        let disposition = value.get("disposition").expect("disposition emitted");
        assert_eq!(disposition.get("source").and_then(JsonValue::as_str), Some("directive"));
        let directive = disposition.get("directive").expect("directive sub-field emitted");
        assert_eq!(
            directive.get("path").and_then(JsonValue::as_str),
            Some("crates/invoice_export/src/config.rs")
        );
        assert_eq!(directive.get("line").and_then(JsonValue::as_u64), Some(17));
        assert!(
            directive
                .get("rationale")
                .and_then(JsonValue::as_str)
                .is_some_and(|r| r.starts_with("internal deploy only")),
            "rationale must round-trip verbatim",
        );
        let parsed: LintFinding = serde_json::from_value(value).expect("round-trip");
        assert_eq!(finding, parsed);
    }

    /// RFC-33a D6: when `disposition` is unset the canonical JSON
    /// MUST omit the key entirely so RFC-28's byte-stable shape for
    /// raw scanner output is preserved.
    #[test]
    fn open_finding_omits_disposition_key() {
        let finding = disposition_fixture(FindingStatus::Open, None);
        let value = serde_json::to_value(&finding).expect("serialise");
        assert_validates(LINT_FINDING_JSON_SCHEMA, &value);
        assert!(
            value.get("disposition").is_none(),
            "canonical JSON must omit the disposition key when unset; got {value}",
        );
        let parsed: LintFinding = serde_json::from_value(value).expect("round-trip");
        assert_eq!(finding, parsed);
    }

    /// RFC-33a D5: a directive whose rationale begins with the
    /// `false-positive:` prefix flips the status to
    /// `false-positive` while still carrying the directive
    /// disposition. Locks the wire spelling and the verbatim
    /// rationale.
    #[test]
    fn false_positive_directive_round_trips() {
        let finding = disposition_fixture(
            FindingStatus::FalsePositive,
            Some(FindingDisposition {
                source: DispositionSource::Directive,
                directive: Some(DirectiveDisposition {
                    path: "crates/invoice_export/src/config.rs".into(),
                    line: 17,
                    rationale: "false-positive: scanner mis-flags the test stub URL".into(),
                }),
                since: None,
            }),
        );
        let value = serde_json::to_value(&finding).expect("serialise");
        assert_validates(LINT_FINDING_JSON_SCHEMA, &value);
        assert_eq!(value.get("status").and_then(JsonValue::as_str), Some("false-positive"));
        let rationale = value
            .pointer("/disposition/directive/rationale")
            .and_then(JsonValue::as_str)
            .expect("rationale present");
        assert!(
            rationale.starts_with("false-positive:"),
            "rationale must lead with the false-positive prefix; got {rationale}",
        );
        let parsed: LintFinding = serde_json::from_value(value).expect("round-trip");
        assert_eq!(finding, parsed);
    }
}
