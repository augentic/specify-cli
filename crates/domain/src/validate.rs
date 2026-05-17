//! Validation rule registry and runner.
//!
//! `Rule` / `CrossRule` declare their `Classification`; [`validate_slice`]
//! returns a `ValidationReport` with `Pass` / `Fail` / `Deferred` results.
//! The report serialises directly via its `serde::Serialize` derive — the
//! kebab-case wire shape (`brief-results`, `cross-checks`, `rule-id`) is
//! produced by the `rename_all = "kebab-case"` attribute on the report and
//! the matching attribute on `ValidationResult`.

use std::collections::BTreeMap;
use std::path::Path;

use specify_error as _; // dependency declared; re-exported via `Error` return type

use crate::capability::PipelineView;
use crate::spec::ParsedSpec;
use crate::task::Progress;

pub mod compatibility;
mod primitives;
mod registry;
mod run;

pub use compatibility::{
    CompatibilityClassification, CompatibilityFinding, CompatibilityReport, CompatibilitySummary,
    classify_project as classify_project_compatibility,
};
pub use registry::{cross_rules, rules_for};
pub use run::validate_slice;

pub use crate::capability::ValidationResult;

/// Structured result of running every applicable rule over a slice dir.
///
/// `brief_results` is keyed by brief id when a brief produces a single
/// artifact (e.g. `"proposal"` → `proposal.md`), or by the artifact path
/// relative to `slice_dir` when the brief's `generates` is a glob
/// matching multiple files (e.g. `"specs/login/spec.md"`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
#[must_use]
pub struct ValidationReport {
    /// Per-brief validation results, keyed by brief id or artifact path.
    pub brief_results: BTreeMap<String, Vec<ValidationResult>>,
    /// Cross-brief validation results.
    pub cross_checks: Vec<ValidationResult>,
    /// `true` when no rule produced a `Fail` outcome.
    pub passed: bool,
}

/// How the CLI decides a rule's outcome — declared at the rule's
/// definition site.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Classification {
    /// CLI decides Pass/Fail deterministically.
    Structural,
    /// CLI always emits `Deferred`; the agent applies judgment.
    Semantic,
}

/// Outcome of invoking a structural rule's `check` function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleOutcome {
    /// The rule passed.
    Pass,
    /// The rule failed with an explanation.
    Fail {
        /// Human-readable failure detail.
        detail: String,
    },
}

/// A named rule attached to a specific brief id.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable dot-namespaced identifier (e.g. `proposal.why-has-content`).
    pub id: &'static str,
    /// Human-readable description of what the rule checks.
    pub description: &'static str,
    /// Whether the rule is structural or semantic.
    pub classification: Classification,
    /// `Some` for `Classification::Structural`; `None` for `Semantic`,
    /// which the runner always materialises as `Deferred`.
    pub check: Option<fn(&BriefContext<'_>) -> RuleOutcome>,
}

/// Inputs a brief-scoped structural checker needs.
#[derive(Debug)]
pub struct BriefContext<'a> {
    /// The brief id being validated.
    pub id: &'a str,
    /// Artifact file content.
    pub content: &'a str,
    /// Parsed spec (when `brief_id == "specs"`).
    pub parsed_spec: Option<&'a ParsedSpec>,
    /// Parsed task progress (when `brief_id == "tasks"`).
    pub tasks: Option<&'a Progress>,
    /// Absolute path to the slice directory.
    pub slice_dir: &'a Path,
    /// Absolute path to the specs directory.
    pub specs_dir: &'a Path,
    /// Schema-inferred terminology (e.g. `"crate"` or `"feature"`).
    pub terminology: &'a str,
}

/// A rule that spans multiple briefs.
#[derive(Debug, Clone, Copy)]
pub struct CrossRule {
    /// Stable dot-namespaced identifier (e.g. `cross.proposal-crates-have-specs`).
    pub id: &'static str,
    /// Human-readable description of what the rule checks.
    pub description: &'static str,
    /// Whether the rule is structural or semantic.
    pub classification: Classification,
    /// Checker function — only invoked for structural rules.
    pub check: fn(&CrossContext<'_>) -> RuleOutcome,
}

/// Inputs a cross-brief checker needs.
#[derive(Debug)]
pub struct CrossContext<'a> {
    /// Absolute path to the slice directory.
    pub slice_dir: &'a Path,
    /// Absolute path to the specs directory.
    pub specs_dir: &'a Path,
    /// Resolved pipeline for the slice.
    pub pipeline: &'a PipelineView,
    /// Schema-inferred terminology.
    pub terminology: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rules_for` returns empty for unknown brief ids.
    #[test]
    fn unknown_brief_no_rules() {
        assert!(rules_for("unknown-brief-id").is_empty());
        assert!(rules_for("").is_empty());
    }

    #[test]
    fn min_rules_per_brief() {
        assert!(rules_for("proposal").len() >= 3);
        assert!(rules_for("specs").len() >= 4);
        assert!(!rules_for("design").is_empty());
        assert!(rules_for("tasks").len() >= 2);
        assert!(!rules_for("composition").is_empty());
        assert!(rules_for("contracts").len() >= 3);
        assert!(cross_rules().len() >= 3);
    }

    /// `ValidationReport` serialises with the canonical kebab-case wire
    /// shape — `passed` / `brief-results` / `cross-checks` at the top,
    /// `status` / `rule-id` / `rule` (+ variant-specific fields) per
    /// result. Pins the derive against accidental rename or reshape.
    #[test]
    fn report_serialises_kebab_case_shape() {
        use crate::capability::ValidationResult;

        let mut brief_results: BTreeMap<String, Vec<ValidationResult>> = BTreeMap::new();
        brief_results.insert(
            "proposal".to_string(),
            vec![ValidationResult::Pass {
                rule_id: "proposal.why-has-content".into(),
                rule: "Has a Why section with at least one sentence".into(),
            }],
        );
        let report = ValidationReport {
            brief_results,
            cross_checks: vec![
                ValidationResult::Fail {
                    rule_id: "cross.design-references-valid".into(),
                    rule: "Every requirement id referenced in design.md exists in specs".into(),
                    detail: "REQ-999 not found".to_string(),
                },
                ValidationResult::Deferred {
                    rule_id: "specs.uses-normative-language".into(),
                    rule: "Uses SHALL/MUST language for normative requirements".into(),
                    reason: "Semantic check — requires LLM judgment",
                },
            ],
            passed: false,
        };

        let value = serde_json::to_value(&report).expect("report serialises");
        assert_eq!(value["passed"], false);
        assert_eq!(value["brief-results"]["proposal"][0]["status"], "pass");
        assert_eq!(value["brief-results"]["proposal"][0]["rule-id"], "proposal.why-has-content");
        assert_eq!(value["cross-checks"][0]["status"], "fail");
        assert_eq!(value["cross-checks"][0]["detail"], "REQ-999 not found");
        assert_eq!(value["cross-checks"][1]["status"], "deferred");
        assert_eq!(value["cross-checks"][1]["reason"], "Semantic check — requires LLM judgment");
    }

    /// Every rule carries a stable `<brief>.<kebab>` id.
    #[test]
    fn rule_ids_are_namespaced() {
        for (brief, prefix) in &[
            ("proposal", "proposal."),
            ("specs", "specs."),
            ("design", "design."),
            ("tasks", "tasks."),
            ("composition", "composition."),
            ("contracts", "contracts."),
        ] {
            for rule in rules_for(brief) {
                assert!(
                    rule.id.starts_with(prefix),
                    "rule id `{}` should start with `{}`",
                    rule.id,
                    prefix
                );
            }
        }
        for rule in cross_rules() {
            assert!(rule.id.starts_with("cross."));
        }
    }
}
