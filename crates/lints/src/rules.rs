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
    ResolveError, ResolveInputs, ResolvedRuleEntry, build_resolved_rules, filter,
    map_resolve_error, resolve, sort_resolved,
};
use serde::{Deserialize, Serialize};
// The structured-finding currency lives in the neutral
// `specify_diagnostics` leaf so the `validate` surface can produce it
// without depending on anything named `lint`. The rules layer
// re-exports it here — both under the neutral names and the legacy
// `Lint*` / `Finding*` aliases — so the codex parser/resolver and the
// existing lint call sites keep resolving while the tree migrates.
pub use specify_diagnostics::{
    Artifact, Confidence, DirectiveDisposition, DispositionSource, FindingDisposition,
    FindingEvidence, FindingLocation, FindingStatus, Severity,
};
pub use specify_diagnostics::{
    Diagnostic as LintFinding, DiagnosticKind, DiagnosticSource as FindingSource,
};

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
    /// Core pack overlay (`adapters/shared/rules/core/`). Excluded
    /// from consumer exports unless `--include-core` is set.
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
/// After C17 every kind is executable: `path-pattern`, `regex`,
/// `schema`, `tool`, `reference-resolves`, `unique`, `set-coverage`,
/// `cardinality`, `constant-eq`, `set-eq`, `content-digest-eq`, and
/// `namespace-owner`. No kind is reserved.
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
    /// Assert that the values some candidate file declares are
    /// exactly equal to a closed expected set — the two-sided
    /// tightening of [`Self::SetCoverage`] (v1 source discriminator:
    /// `adapter-briefs-equal-operations`).
    SetEq,
    /// Assert that the content digest (SHA-256) of one file equals an
    /// expected digest (v1 source discriminator:
    /// `agent-teams-match-canonical`).
    ContentDigestEq,
    /// Assert that each rule file's id-namespace prefix is authored
    /// only under the directory that owns that namespace (v1 source
    /// discriminator: `rule-namespace-matches-owner`).
    NamespaceOwner,
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

#[cfg(test)]
mod tests {
    use jsonschema::Validator;
    use serde_json::Value as JsonValue;
    use specify_schema::{RESOLVED_RULES_JSON_SCHEMA, RULE_JSON_SCHEMA};

    use super::{
        Applicability, Deprecated, DeterministicHint, HintKind, LintMode, Origin, PathRoot,
        Reference, ResolvedRule, ResolvedRules, Rule, Severity,
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
    fn resolved_codex_round_trips() {
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
    fn deprecated_replaced_by_kebab() {
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
}
