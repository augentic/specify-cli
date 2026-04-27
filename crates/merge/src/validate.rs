//! Post-merge coherence checks — port of Python `validate_baseline`
//! (archived Python reference, `validate_baseline` lines 298–352).

use std::collections::HashSet;

use regex::Regex;
use specify_schema::ValidationResult;
use specify_spec::{
    REQUIREMENT_ID_PATTERN, REQUIREMENT_ID_PREFIX, SCENARIO_HEADING, parse_baseline,
};

const RULE_NO_DUPLICATE_IDS: &str = "merge.no-duplicate-ids";
const RULE_NO_DUPLICATE_IDS_DESC: &str = "baseline has no duplicate requirement IDs";

const RULE_NO_DUPLICATE_NAMES: &str = "merge.no-duplicate-names";
const RULE_NO_DUPLICATE_NAMES_DESC: &str = "baseline has no duplicate requirement names";

const RULE_REQ_HAS_ID: &str = "merge.requirement-has-id";
const RULE_REQ_HAS_ID_DESC: &str = "every requirement block carries an `ID:` line";

const RULE_ID_MATCHES_PATTERN: &str = "merge.id-matches-pattern";
const RULE_ID_MATCHES_PATTERN_DESC: &str = "requirement IDs match the canonical REQ-NNN pattern";

const RULE_REQ_HAS_SCENARIO: &str = "merge.requirement-has-scenario";
const RULE_REQ_HAS_SCENARIO_DESC: &str = "every requirement declares at least one scenario";

const RULE_DESIGN_REFS_EXIST: &str = "merge.design-references-exist";
const RULE_DESIGN_REFS_EXIST_DESC: &str =
    "requirement IDs referenced by design.md exist in the baseline";

/// Post-merge coherence validation.
///
/// Returns one [`ValidationResult::Fail`] per violation; an empty vec means
/// the baseline passes every coherence rule. The `design` argument enables
/// the orphan-reference rule (`merge.design-references-exist`) — see the
/// inline parity quirk below.
///
/// # Panics
///
/// Panics if `REQUIREMENT_ID_PATTERN` is not a valid regex (compile-time
/// constant — should never happen).
#[must_use]
pub fn validate_baseline(baseline: &str, design: Option<&str>) -> Vec<ValidationResult> {
    let parsed = parse_baseline(baseline);
    let blocks = parsed.requirements;
    let mut results: Vec<ValidationResult> = Vec::new();

    // (a) Duplicate IDs.
    let mut seen_ids: HashSet<&str> = HashSet::new();
    for block in &blocks {
        if block.id.is_empty() {
            continue;
        }
        if seen_ids.contains(block.id.as_str()) {
            results.push(ValidationResult::Fail {
                rule_id: RULE_NO_DUPLICATE_IDS,
                rule: RULE_NO_DUPLICATE_IDS_DESC,
                detail: format!("Duplicate ID: {}", block.id),
            });
        }
        seen_ids.insert(block.id.as_str());
    }

    // (b) Duplicate names.
    let mut seen_names: HashSet<&str> = HashSet::new();
    for block in &blocks {
        if seen_names.contains(block.name.as_str()) {
            results.push(ValidationResult::Fail {
                rule_id: RULE_NO_DUPLICATE_NAMES,
                rule: RULE_NO_DUPLICATE_NAMES_DESC,
                detail: format!("Duplicate requirement name: {}", block.name),
            });
        }
        seen_names.insert(block.name.as_str());
    }

    // (c) Heading structure — ID present, ID matches pattern, scenario present.
    let id_pattern =
        Regex::new(REQUIREMENT_ID_PATTERN).expect("REQUIREMENT_ID_PATTERN must be a valid regex");
    // The scenario check in Python strips the trailing colon so the
    // substring match still fires when body text contains `"#### Scenario"`
    // without a colon; preserve that.
    let scenario_needle = SCENARIO_HEADING.trim_end_matches(':');
    for block in &blocks {
        if block.id.is_empty() {
            results.push(ValidationResult::Fail {
                rule_id: RULE_REQ_HAS_ID,
                rule: RULE_REQ_HAS_ID_DESC,
                detail: format!(
                    "Requirement '{}' has no {} line",
                    block.name, REQUIREMENT_ID_PREFIX
                ),
            });
        } else if !id_pattern.is_match(&block.id) {
            results.push(ValidationResult::Fail {
                rule_id: RULE_ID_MATCHES_PATTERN,
                rule: RULE_ID_MATCHES_PATTERN_DESC,
                detail: format!(
                    "Requirement '{}' has invalid ID '{}' (expected pattern: {})",
                    block.name, block.id, REQUIREMENT_ID_PATTERN
                ),
            });
        }
        if !block.body.contains(scenario_needle) {
            results.push(ValidationResult::Fail {
                rule_id: RULE_REQ_HAS_SCENARIO,
                rule: RULE_REQ_HAS_SCENARIO_DESC,
                detail: format!(
                    "Requirement '{}' ({}) has no {} section",
                    block.name, block.id, SCENARIO_HEADING
                ),
            });
        }
    }

    // (d) Orphan design references.
    if let Some(design_text) = design {
        // Parity quirk: Python's regex is anchored with ^...$ but lacks
        // re.MULTILINE, so finditer() on a multi-line design string never
        // matches. `specify-validate` (Change G, rule
        // `cross.design-references-valid`) will implement a correct
        // multi-line check. `REQUIREMENT_ID_PATTERN` itself already
        // contains ^ and $ — Rust's default `Regex` treats them as
        // string boundaries (no MULTILINE flag), so we match Python
        // byte-for-byte by feeding the constant straight to the engine.
        let ref_pattern =
            Regex::new(REQUIREMENT_ID_PATTERN).expect("REQUIREMENT_ID_PATTERN must compile");
        let baseline_ids: HashSet<&str> = seen_ids.iter().copied().collect();
        for m in ref_pattern.find_iter(design_text) {
            let ref_id = m.as_str();
            if !baseline_ids.contains(ref_id) {
                results.push(ValidationResult::Fail {
                    rule_id: RULE_DESIGN_REFS_EXIST,
                    rule: RULE_DESIGN_REFS_EXIST_DESC,
                    detail: format!("Design references {ref_id} which does not exist in baseline"),
                });
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_reference_regex_is_compilable() {
        // If this ever fails, the const changed and the expect() inside
        // `validate_baseline` would start panicking in the wild.
        Regex::new(REQUIREMENT_ID_PATTERN).unwrap();
    }

    #[test]
    fn ok_baseline_yields_no_failures() {
        let baseline = "### Requirement: A\n\nID: REQ-001\n\n#### Scenario: x\n\n- ok\n";
        assert!(validate_baseline(baseline, None).is_empty());
    }

    #[test]
    fn duplicate_ids_produce_single_fail() {
        let baseline = "### Requirement: A\n\nID: REQ-001\n\n#### Scenario: x\n\n- ok\n\n### Requirement: B\n\nID: REQ-001\n\n#### Scenario: y\n\n- ok\n";
        let results = validate_baseline(baseline, None);
        let fails: Vec<_> = results.iter().filter_map(as_fail).collect();
        assert!(
            fails.iter().any(|(rid, detail)| *rid == RULE_NO_DUPLICATE_IDS
                && detail.contains("Duplicate ID: REQ-001")),
            "expected duplicate-id fail, got {fails:?}"
        );
    }

    #[test]
    fn missing_id_and_invalid_pattern_rules_fire() {
        let invalid = "### Requirement: A\n\nID: NOT-AN-ID\n\n#### Scenario: x\n\n- ok\n\n### Requirement: B\n\n#### Scenario: y\n\n- ok\n";
        let results = validate_baseline(invalid, None);
        let fails: Vec<_> = results.iter().filter_map(as_fail).collect();
        assert!(
            fails.iter().any(|(rid, _)| *rid == RULE_ID_MATCHES_PATTERN),
            "expected id-matches-pattern fail, got {fails:?}"
        );
        assert!(
            fails.iter().any(|(rid, _)| *rid == RULE_REQ_HAS_ID),
            "expected requirement-has-id fail, got {fails:?}"
        );
    }

    #[test]
    fn design_refs_parity_quirk_never_matches_multiline() {
        // Python's un-anchored-but-no-MULTILINE regex never fires on a
        // multi-line string; we must preserve that behaviour here. When
        // Change G lands `cross.design-references-valid`, that *will*
        // catch REQ-999 below.
        let baseline = "### Requirement: A\n\nID: REQ-001\n\n#### Scenario: ok\n\n- ok\n";
        let design = "Design references REQ-999 which is orphan.\nAnother line.\n";
        assert!(validate_baseline(baseline, Some(design)).is_empty());
    }

    fn as_fail(result: &ValidationResult) -> Option<(&'static str, &str)> {
        match result {
            ValidationResult::Fail { rule_id, detail, .. } => Some((*rule_id, detail.as_str())),
            _ => None,
        }
    }
}
