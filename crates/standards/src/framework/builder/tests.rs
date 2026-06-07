use specify_diagnostics::Severity;

use super::{core_id_for, severity_for};

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

/// The codex id table is empty: no imperative
/// authoring predicate runs as a producer, so every id falls through to
/// the `rule_id: None` form. CORE-009 / CORE-026 now run through the
/// `rules` WASI tool, which stamps its own codex ids on the wire.
#[test]
fn codex_id_table_is_empty() {
    assert!(core_id_for("rules.namespace-ownership-violation").is_none());
    assert!(core_id_for("rules.duplicate-rule-id").is_none());
    assert!(core_id_for("adapter.missing-manifest").is_none());
}
