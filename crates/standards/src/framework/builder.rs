//! Shared builder bridging the imperative framework `Check` predicates
//! to the canonical [`Diagnostic`] currency.
//!
//! Every predicate constructs its findings through
//! [`framework_finding`], which is the former binary-boundary
//! `map_one` mapper minus the `id` / `fingerprint` assignment (those
//! are stamped by the finalize pass in [`crate::framework::check::run`]
//! once the deduplicated order is known — the fingerprint preimage
//! excludes `id`, so deferring both is safe). The unification deleted
//! the lightweight `specify_authoring::Finding` / `Location` types: the
//! predicates speak [`Diagnostic`] end-to-end.
//!
//! ## Decision: imperative `rule_id` mapped onto the closed codex `rule-id`
//!
//! A predicate carries a static authoring identifier such as
//! `rules.schema-violation`, `skill.unknown-tool`, or
//! `links.broken-reference`. The diagnostic schema constrains `rule-id`
//! to the closed codex regex
//! `^(UNI|SRC|FRAME|CORE|RUST|IFACE|SEC|OMNIA|VECTIS|ORG)-[0-9]{3}$`, so
//! `CORE_ID_TABLE` assigns every still-active predicate a `CORE-NNN`
//! id. A mapped finding sets `rule_id: Some("CORE-NNN")` and emits a
//! clean `title`; an unmapped id falls back to `rule_id: None` with the
//! legacy `[rule_id]` title prefix so a newly-added predicate is never
//! silently dropped from the wire.

use std::path::Path;

use specify_diagnostics::{
    Artifact, Diagnostic, DiagnosticKind, DiagnosticSource, FindingEvidence, FindingLocation,
    Severity,
};
use specify_digest::sha256_hex;

/// Mapping from each still-active imperative authoring rule id to its
/// closed codex `CORE-NNN` id.
///
/// `CORE-001..009` are owned by declarative rule files in the framework
/// repo (`adapters/shared/rules/core/`). `rules.namespace-ownership-violation`
/// reuses `CORE-009` (its declarative counterpart); every other entry
/// is minted at `CORE-010` and up.
const CORE_ID_TABLE: &[(&str, &str)] = &[
    ("rules.namespace-ownership-violation", "CORE-009"),
    ("adapter.missing-manifest", "CORE-010"),
    ("adapter.execution-agent", "CORE-051"),
    ("agent-teams.missing-canonical", "CORE-011"),
    ("agent-teams.non-canonical-overlay", "CORE-012"),
    ("brief.exceeds-size-limit", "CORE-013"),
    ("brief.frontmatter-forbidden", "CORE-014"),
    ("docs.missing-diagram-asset", "CORE-015"),
    ("docs.specify-history-citation-in-docs", "CORE-016"),
    ("docs.text-pipeline-diagram", "CORE-017"),
    ("links.brief-schema-link-resolve", "CORE-018"),
    ("links.broken-reference", "CORE-019"),
    ("links.unresolved-directive", "CORE-020"),
    ("plugins.broken-symlink", "CORE-021"),
    ("plugins.marketplace-drift", "CORE-022"),
    ("prose.invocation-positional", "CORE-023"),
    ("prose.numeric-cap-exceeded", "CORE-024"),
    ("prose.operational-vocabulary", "CORE-025"),
    ("rules.duplicate-rule-id", "CORE-026"),
    ("rules.schema-violation", "CORE-027"),
    ("scenarios.artifact-path-unsafe", "CORE-028"),
    ("scenarios.body-id-mismatch", "CORE-029"),
    ("scenarios.duplicate-id", "CORE-030"),
    ("scenarios.recorded-trace-violation", "CORE-031"),
    ("scenarios.schema-violation", "CORE-032"),
    ("scenarios.stages-not-contiguous-prefix", "CORE-033"),
    ("scenarios.stale-recorded-trace", "CORE-034"),
    ("skill.argument-hint-grammar", "CORE-035"),
    ("skill.description-grammar", "CORE-036"),
    ("skill.envelope-json-in-body", "CORE-037"),
    ("skill.frontmatter-restatement", "CORE-038"),
    ("skill.inline-json-too-long", "CORE-039"),
    ("skill.invalid-critical-path", "CORE-040"),
    ("skill.missing-critical-path", "CORE-041"),
    ("skill.missing-frontmatter", "CORE-042"),
    ("skill.name-directory-mismatch", "CORE-043"),
    ("skill.schema-violation", "CORE-044"),
    ("skill.section-line-count", "CORE-045"),
    ("skill.step-body-duplicates-critical-path", "CORE-046"),
    ("skill.unknown-tool", "CORE-047"),
    ("skill.variable-coverage", "CORE-048"),
    ("tools.invalid-declaration", "CORE-049"),
    ("tools.invocation-not-equivalent", "CORE-050"),
];

/// Resolve the closed codex `CORE-NNN` id for an imperative authoring
/// rule id, or `None` when the id has not been assigned one yet (the
/// caller falls back to the `[...]` title-prefix form).
#[must_use]
pub fn core_id_for(rule_id: &str) -> Option<&'static str> {
    CORE_ID_TABLE.iter().find(|(authoring, _)| *authoring == rule_id).map(|(_, core)| *core)
}

/// 16 `KiB` cap on the serialised evidence object per the rules
/// contract (mirror of `specify_diagnostics`'s `EVIDENCE_MAX_BYTES`,
/// kept local so the builder does not import the validator).
const EVIDENCE_MAX_BYTES: usize = 16 * 1024;

/// Headroom reserved for JSON framing on top of the raw message bytes;
/// 1 `KiB` of slack keeps the snippet path safe for any realistic
/// authoring message while still letting the Digest fallback exercise
/// the boundary.
const EVIDENCE_MARGIN_BYTES: usize = 1024;

/// Soft cap on the synthesised title. The schema imposes only
/// `minLength: 1`, but pinning a producer-side ceiling keeps PR comment
/// / dashboard rendering predictable.
const TITLE_MAX_CHARS: usize = 200;

/// Map an authoring `rule_id` to the closed review [`Severity`] enum.
///
/// `rules.schema-violation` is elevated to `Critical`: a malformed rule
/// file breaks every downstream consumer of the resolved codex.
/// `adapter.execution-agent` is demoted to `Suggestion` (RFC-29 D9): a
/// first-party adapter running via `agent` is informational, not a
/// defect, so it must never block CI. Every other authoring rule maps
/// to the `Important` default.
#[must_use]
pub fn severity_for(rule_id: &str) -> Severity {
    match rule_id {
        "rules.schema-violation" => Severity::Critical,
        "adapter.execution-agent" => Severity::Suggestion,
        _ => Severity::Important,
    }
}

/// Build a [`FindingLocation`] from a path, a 1-based line, and an
/// optional column. Normalises back-slashes to forward-slashes so the
/// path stays wire-shaped before the finalize pass rebases it.
#[must_use]
pub fn loc(path: impl AsRef<Path>, line: usize, column: Option<usize>) -> FindingLocation {
    FindingLocation {
        path: path.as_ref().to_string_lossy().replace('\\', "/"),
        line: Some(usize_to_u32(line)),
        column: column.map(usize_to_u32),
        end_line: None,
        end_column: None,
    }
}

/// Build an unfinalised [`Diagnostic`] for an imperative authoring
/// finding. `id` and `fingerprint` are left empty for
/// [`crate::framework::check::run`] to stamp once the deduplicated
/// order is known.
#[must_use]
pub fn framework_finding(
    rule_id: &str, message: String, location: Option<FindingLocation>,
) -> Diagnostic {
    let core = core_id_for(rule_id);
    let title = build_title(rule_id, &message, core.is_some());
    let evidence = build_evidence(&message);
    Diagnostic {
        id: String::new(),
        rule_id: core.map(str::to_string),
        related_rule_ids: None,
        title,
        severity: severity_for(rule_id),
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter: None,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Unknown,
        location,
        evidence,
        impact: format!("Authoring check '{rule_id}' failed."),
        remediation: format!(
            "Resolve the violation reported by '{rule_id}'. See the finding message for details."
        ),
        confidence: None,
        fingerprint: String::new(),
        status: None,
        disposition: None,
    }
}

/// Read the verbatim [`FindingEvidence::Snippet`] payload of a finding,
/// or the empty string for the digest / structured variants. Used by
/// the framework integration tests to assert on a predicate's message.
#[must_use]
pub fn snippet(finding: &Diagnostic) -> &str {
    match &finding.evidence {
        FindingEvidence::Snippet { value } => value,
        FindingEvidence::Digest { .. } | FindingEvidence::Structured { .. } => "",
    }
}

fn build_title(rule_id: &str, message: &str, has_core_id: bool) -> String {
    let head = message.lines().find(|line| !line.trim().is_empty()).unwrap_or(message);
    let head = head.trim();
    let body = if head.is_empty() { "(no message)" } else { head };
    // When the finding carries a closed codex `rule_id`, the id is
    // already wire-visible on its own field, so the title stays clean.
    // Unmapped predicates keep the `[rule_id]` prefix so the imperative
    // id remains greppable while `rule_id` is `None`.
    let raw = if has_core_id { body.to_owned() } else { format!("[{rule_id}] {body}") };
    truncate_chars(&raw, TITLE_MAX_CHARS)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_owned();
    }
    let kept: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn build_evidence(message: &str) -> FindingEvidence {
    if message.len() <= EVIDENCE_MAX_BYTES - EVIDENCE_MARGIN_BYTES {
        FindingEvidence::Snippet {
            value: message.to_owned(),
        }
    } else {
        let sha256 = sha256_hex(message.as_bytes());
        let summary = format!(
            "authoring finding message digested ({} bytes); full message available via sha256",
            message.len(),
        );
        FindingEvidence::Digest {
            sha256,
            summary,
            locations: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use specify_diagnostics::Severity;

    use super::{core_id_for, severity_for};
    use crate::framework::check::skill_frontmatter::{
        RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR, RULE_MISSING_FRONTMATTER,
        RULE_NAME_DIRECTORY_MISMATCH, RULE_UNKNOWN_TOOL,
    };
    use crate::framework::check::{
        RULE_DUPLICATE_RULE_ID, RULE_MISSING_MANIFEST, RULE_NAMESPACE_OWNERSHIP_VIOLATION,
        RULE_RECORDED_TRACE_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS, RULE_STALE_RECORDED_TRACE,
        SCENARIO_RULE_ARTIFACT_PATH_UNSAFE, SCENARIO_RULE_BODY_ID_MISMATCH,
        SCENARIO_RULE_DUPLICATE_ID, SCENARIO_RULE_SCHEMA_VIOLATION, SKILL_RULE_SCHEMA_VIOLATION,
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
        for rule_id in [
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
    fn unclassified_defaults_important() {
        assert_eq!(severity_for("future.unmapped-rule"), Severity::Important);
        assert_eq!(severity_for(""), Severity::Important);
        assert_eq!(severity_for("totally.made.up"), Severity::Important);
    }

    /// Every `RULE_*` constant re-exported from
    /// [`crate::framework::check`] resolves to a known severity and a
    /// closed `CORE-NNN` id.
    #[test]
    fn exported_rules_map_to_severity_and_core_id() {
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
}
