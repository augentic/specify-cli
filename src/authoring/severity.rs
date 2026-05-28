//! Maps a `specify_authoring::Finding`'s rule-id-driven imperative
//! finding to the closed [`Severity`] enum.
//!
//! The framework-convergence contract requires every authoring
//! finding emitted by `specdev check --format json` to carry a closed
//! severity from `{critical, important, suggestion, optional}`. The
//! authoring `Finding` type does not currently carry a severity field
//! (`crates/authoring/src/finding.rs` exposes only `rule_id`,
//! `message`, and `location`); the mapping is therefore by
//! rule-id family, with a [`Severity::Important`] default for any
//! unclassified rule id.
//!
//! ## Authoring rule-id → review severity table
//!
//! ```text
//! | Authoring rule-id                                | review severity |
//! | ------------------------------------------------ | --------------- |
//! | rules.schema-violation                           | critical        |
//! | adapter.*                                        | important       |
//! | agent-teams.*                                    | important       |
//! | brief.*                                          | important       |
//! | rules.duplicate-rule-id                          | important       |
//! | rules.namespace-ownership-violation              | important       |
//! | docs.*                                           | important       |
//! | links.*                                          | important       |
//! | plugins.*                                        | important       |
//! | prose.*                                          | important       |
//! | scenarios.*                                      | important       |
//! | skill.*                                          | important       |
//! | tools.*                                          | important       |
//! | (default for unclassified rule ids)              | important       |
//! ```
//!
//! ### Calibration
//!
//! Only `rules.schema-violation` is elevated to `Critical`: a malformed
//! rule file is a fundamental schema breakage that breaks every
//! downstream consumer of the resolved codex (`specrun rules export`,
//! review tooling, target adapter overlays). Every other authoring
//! rule — duplicate rule ids, namespace-ownership violations, skill /
//! brief / link / docs / scenarios / tools / agent-teams families —
//! is an authoring mistake the framework wants fixed but does not
//! itself break consumers, and maps to `Important`. Future RFCs may
//! elevate additional families (e.g. `*.schema-violation` across
//! authoring surfaces); the default keeps the table stable until then.
//!
//! ## Layering
//!
//! This module deliberately lives at the binary boundary
//! (`src/authoring/severity.rs`) and not in the `specify-authoring`
//! library crate. Per the framework-authoring mapping contract,
//! the CH-21 `Finding` → `LintFinding` mapper sits at the
//! `specdev` binary boundary so `specify-authoring` does not take a
//! dependency on `specify-domain`. CH-20 is a building block for that
//! mapper, so the severity table must live in the binary layer too.

use specify_lints::Severity;

/// Map an authoring `Finding::rule_id` to the closed review
/// [`Severity`] enum.
///
/// Unknown rule ids fall through to [`Severity::Important`] — the
/// default escalation level documented for adapter overlays in
/// `ResolvedRules` export contract.
#[must_use]
pub fn severity_for(rule_id: &str) -> Severity {
    match rule_id {
        "rules.schema-violation" => Severity::Critical,
        _ => Severity::Important,
    }
}

#[cfg(test)]
mod tests {
    use specify_authoring::check::{
        RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR, RULE_DUPLICATE_NAME,
        RULE_DUPLICATE_RULE_ID, RULE_MISSING_FRONTMATTER, RULE_MISSING_MANIFEST,
        RULE_NAME_DIRECTORY_MISMATCH, RULE_NAMESPACE_OWNERSHIP_VIOLATION,
        RULE_RECORDED_TRACE_VIOLATION, RULE_SCHEMA_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS,
        RULE_STALE_RECORDED_TRACE, RULE_UNKNOWN_TOOL, SCENARIO_RULE_ARTIFACT_PATH_UNSAFE,
        SCENARIO_RULE_BODY_ID_MISMATCH, SCENARIO_RULE_DUPLICATE_ID, SCENARIO_RULE_SCHEMA_VIOLATION,
        SKILL_RULE_SCHEMA_VIOLATION,
    };
    use specify_lints::Severity;

    use super::severity_for;

    /// `rules.schema-violation` is the one rule the table elevates to
    /// `Critical` (`ResolvedRules` export contract — schema breakage
    /// blocks every downstream consumer of the resolved codex).
    #[test]
    fn codex_schema_violation_maps_to_critical() {
        assert_eq!(severity_for("rules.schema-violation"), Severity::Critical);
    }

    /// The `skill.*` family covers frontmatter and body checks; per the
    /// table, every member maps to `Important`.
    #[test]
    fn skill_family_maps_to_important() {
        for rule_id in [
            "skill.schema-violation",
            "skill.missing-frontmatter",
            "skill.name-directory-mismatch",
            "skill.duplicate-name",
            "skill.unknown-tool",
            "skill.description-grammar",
            "skill.argument-hint-grammar",
            "skill.body-line-count",
            "skill.section-line-count",
            "skill.missing-critical-path",
            "skill.invalid-critical-path",
            "skill.inline-json-too-long",
            "skill.envelope-json-in-body",
            "skill.step-body-duplicates-critical-path",
            "skill.frontmatter-restatement",
            "skill.variable-coverage",
        ] {
            assert_eq!(severity_for(rule_id), Severity::Important, "{rule_id}");
        }
    }

    /// The `links.*` family is a documentation-quality gate; per the
    /// table, every member maps to `Important`.
    #[test]
    fn links_family_maps_to_important() {
        for rule_id in [
            "links.unresolved",
            "links.broken-reference",
            "links.unresolved-directive",
            "links.brief-schema-link-resolve",
        ] {
            assert_eq!(severity_for(rule_id), Severity::Important, "{rule_id}");
        }
    }

    /// Unknown / unclassified rule ids fall through to the documented
    /// `Important` default.
    #[test]
    fn unclassified_rule_id_defaults_to_important() {
        assert_eq!(severity_for("future.unmapped-rule"), Severity::Important);
        assert_eq!(severity_for(""), Severity::Important);
        assert_eq!(severity_for("totally.made.up"), Severity::Important);
    }

    /// Every `RULE_*` constant re-exported from
    /// `specify_authoring::check` resolves to a known severity. The
    /// per-axis assertions above pin the elevated cases; this test
    /// pins the broader contract that no exported constant escapes
    /// the mapper. The codex `RULE_SCHEMA_VIOLATION` constant is not
    /// re-exported by the crate (it collides with the adapter /
    /// skill / scenarios constants of the same name); the Critical
    /// arm is covered by `codex_schema_violation_maps_to_critical`
    /// above.
    #[test]
    fn every_exported_rule_constant_maps_to_a_known_severity() {
        let important = [
            // adapter
            RULE_SCHEMA_VIOLATION,
            RULE_MISSING_MANIFEST,
            // codex
            RULE_DUPLICATE_RULE_ID,
            RULE_NAMESPACE_OWNERSHIP_VIOLATION,
            // skill frontmatter
            RULE_ARGUMENT_HINT_GRAMMAR,
            RULE_DESCRIPTION_GRAMMAR,
            RULE_DUPLICATE_NAME,
            RULE_MISSING_FRONTMATTER,
            RULE_NAME_DIRECTORY_MISMATCH,
            SKILL_RULE_SCHEMA_VIOLATION,
            RULE_UNKNOWN_TOOL,
            // scenarios
            SCENARIO_RULE_ARTIFACT_PATH_UNSAFE,
            SCENARIO_RULE_BODY_ID_MISMATCH,
            SCENARIO_RULE_DUPLICATE_ID,
            SCENARIO_RULE_SCHEMA_VIOLATION,
            RULE_RECORDED_TRACE_VIOLATION,
            RULE_STAGES_NOT_CONTIGUOUS,
            RULE_STALE_RECORDED_TRACE,
        ];

        for rule_id in important {
            assert_eq!(
                severity_for(rule_id),
                Severity::Important,
                "{rule_id} must map to Important",
            );
        }
    }
}
