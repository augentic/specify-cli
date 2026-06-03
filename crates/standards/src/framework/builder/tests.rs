use specify_diagnostics::Severity;

use super::{core_id_for, severity_for};
use crate::framework::check::skill_frontmatter::{
    RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR, RULE_MISSING_FRONTMATTER,
    RULE_NAME_DIRECTORY_MISMATCH, RULE_UNKNOWN_TOOL,
};
use crate::framework::check::{
    RULE_DUPLICATE_RULE_ID, RULE_MISSING_MANIFEST, RULE_NAMESPACE_OWNERSHIP_VIOLATION,
    RULE_RECORDED_TRACE_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS, RULE_STALE_RECORDED_TRACE,
    SCENARIO_RULE_ARTIFACT_PATH_UNSAFE, SCENARIO_RULE_BODY_ID_MISMATCH, SCENARIO_RULE_DUPLICATE_ID,
    SCENARIO_RULE_SCHEMA_VIOLATION, SKILL_RULE_SCHEMA_VIOLATION,
};

/// `rules.schema-violation` is the one rule the table elevates to
/// `Critical` — schema breakage blocks every downstream consumer of
/// the resolved codex.
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
        "skill.unknown-tool",
        "skill.description-grammar",
        "skill.argument-hint-grammar",
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
    for rule_id in
        ["links.broken-reference", "links.unresolved-directive", "links.brief-schema-link-resolve"]
    {
        assert_eq!(severity_for(rule_id), Severity::Important, "{rule_id}");
    }
}

/// Unknown / unclassified rule ids fall through to the documented
/// `Important` default.
#[test]
fn unclassified_defaults_important() {
    assert_eq!(severity_for("future.unmapped-rule"), Severity::Important);
    assert_eq!(severity_for(""), Severity::Important);
    assert_eq!(severity_for("totally.made.up"), Severity::Important);
}

/// Every `RULE_*` constant re-exported from
/// [`crate::framework::check`] resolves to a known severity and a
/// closed `CORE-NNN` id.
#[test]
fn exported_rules_map_severity_core_id() {
    let important = [
        RULE_MISSING_MANIFEST,
        RULE_DUPLICATE_RULE_ID,
        RULE_NAMESPACE_OWNERSHIP_VIOLATION,
        RULE_ARGUMENT_HINT_GRAMMAR,
        RULE_DESCRIPTION_GRAMMAR,
        RULE_MISSING_FRONTMATTER,
        RULE_NAME_DIRECTORY_MISMATCH,
        SKILL_RULE_SCHEMA_VIOLATION,
        RULE_UNKNOWN_TOOL,
        SCENARIO_RULE_ARTIFACT_PATH_UNSAFE,
        SCENARIO_RULE_BODY_ID_MISMATCH,
        SCENARIO_RULE_DUPLICATE_ID,
        SCENARIO_RULE_SCHEMA_VIOLATION,
        RULE_RECORDED_TRACE_VIOLATION,
        RULE_STAGES_NOT_CONTIGUOUS,
        RULE_STALE_RECORDED_TRACE,
    ];

    for rule_id in important {
        assert_eq!(severity_for(rule_id), Severity::Important, "{rule_id} severity");
        assert!(core_id_for(rule_id).is_some(), "{rule_id} must map to a CORE id");
    }
}
