//! Map a `specify_authoring::Finding` to a structured lint finding
//! [`LintFinding`].
//!
//! This module deliberately lives at the binary boundary
//! (`src/authoring/map_finding.rs`) alongside CH-20's
//! [`crate::authoring::severity`]. Per the rules contract §"Relationship to
//! framework authoring", the `specify-authoring` library MUST NOT
//! depend on `specify-workflow`; the structured-finding mapper therefore
//! has to live in the binary crate so the `specdev` JSON export can
//! reach across both worlds without polluting the authoring layer's
//! dependency graph.
//!
//! ## Mapping table
//!
//! | `Finding` source                       | `LintFinding` field                  |
//! | -------------------------------------- | -------------------------------------- |
//! | sequence index (1-based, 4-digit)      | `id` = `"FIND-{NNNN}"`                 |
//! | `rule_id` (authoring imperative)       | `title` prefix `"[{rule_id}] ..."`     |
//! | first non-empty line of `message`      | `title` body                           |
//! | (see decision below)                   | `rule_id` = `None`                     |
//! | `severity::severity_for(rule_id)`      | `severity`                             |
//! | (deterministic producer)               | `source` = `FindingSource::Deterministic` |
//! | (framework-internal)                   | `target_adapter` / `source_adapter` / `slice` / `change` = `None` |
//! | (framework-internal)                   | `artifact` = `Artifact::Unknown`       |
//! | `location` (path normalised, line/column widened) | `location`                  |
//! | `message`                              | `evidence` (`Snippet`; or `Digest` for oversize) |
//! | derived per `rule_id`                  | `impact` / `remediation`               |
//! | (deterministic producer)               | `confidence` = `None`                  |
//! | `fingerprint::fingerprint(&self)`      | `fingerprint`                          |
//! | (raw scanner output)                   | `status` = `None`                      |
//! | (raw scanner output)                   | `disposition` = `None`                 |
//!
//! ### Decision: imperative `rule_id` mapped onto the closed codex `rule-id`
//!
//! `crates/authoring/src/finding.rs` returns a static authoring
//! identifier such as `rules.schema-violation`, `skill.unknown-tool`,
//! or `links.broken-reference`. The wire schema at
//! `schemas/lint/finding.schema.json` constrains `rule-id` to the
//! closed codex regex
//! `^(UNI|SRC|FRAME|CORE|RUST|IFACE|SEC|OMNIA|VECTIS|ORG)-[0-9]{3}$`.
//!
//! Every still-active imperative predicate is therefore assigned a
//! `CORE-NNN` id by `CORE_ID_TABLE`. The mapper sets
//! `rule_id: Some("CORE-NNN")` and emits a clean `title` (the first
//! non-empty `message` line, no `[...]` prefix). Predicates whose
//! declarative counterpart already owns a `CORE-*` id reuse that id so
//! the migration-overlap dedupe (RFC-34 §F5) collapses the duplicate —
//! `rules.namespace-ownership-violation` reuses `CORE-009`.
//!
//! The numbering above the framework-allocated `CORE-001..009` block is
//! a fresh sequential assignment minted in this crate (operator choice;
//! see the plan). It carries a forward-collision risk against the
//! framework repo's `adapters/shared/rules/core/` catalog as more
//! predicates migrate to declarative rules; reconcile when those rule
//! files land.
//!
//! Any rule id absent from `CORE_ID_TABLE` falls back to
//! `rule_id: None` with the legacy `[...]` title prefix so a
//! newly-added predicate is never silently dropped from the wire.

use specify_authoring::finding::{Finding, Location};
use specify_lints::fingerprint::fingerprint;
use specify_lints::{
    Artifact, DiagnosticKind, FindingEvidence, FindingLocation, FindingSource, LintFinding,
};
use specify_tool::sha256_hex;

use crate::authoring::severity::severity_for;

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

/// 16 `KiB` cap on the serialised evidence object per the rules contract (mirror
/// of `specify_lints::finding::EVIDENCE_MAX_BYTES`, kept
/// local so the mapper does not import the validator).
const EVIDENCE_MAX_BYTES: usize = 16 * 1024;

/// Headroom reserved for JSON framing on top of the raw message
/// bytes (`{"kind":"snippet","value":""}` is 29 bytes and JSON
/// escaping a worst-case payload can roughly double the byte count
/// for control-heavy text). 1 `KiB` of slack keeps the snippet path
/// safe for any realistic authoring message while still letting the
/// Digest fallback test exercise the boundary.
const EVIDENCE_MARGIN_BYTES: usize = 1024;

/// Soft cap on the synthesised title. The schema imposes only
/// `minLength: 1`, but pinning a producer-side ceiling keeps PR
/// comment / dashboard rendering predictable.
const TITLE_MAX_CHARS: usize = 200;

/// Map a single authoring [`Finding`] to a [`LintFinding`] with id
/// `FIND-0001`.
///
/// Equivalent to `map_findings(&[input.clone()]).into_iter().next()`
/// but avoids the allocation. The fingerprint is computed last so
/// every other field is hashed exactly as serialised.
#[must_use]
pub fn map_finding(input: &Finding) -> LintFinding {
    map_one(input, 1)
}

/// Map a batch of authoring [`Finding`]s to [`LintFinding`]s,
/// assigning sequential `FIND-{NNNN}` ids in input order (1-based,
/// 4-digit zero-padded).
///
/// The sequence is producer-local: it MUST NOT be assumed stable
/// across runs because reordering in upstream `Check::run`
/// implementations will shuffle the ids. Callers that need a stable
/// dedup key SHOULD use [`LintFinding::fingerprint`] instead.
#[must_use]
pub fn map_findings(inputs: &[Finding]) -> Vec<LintFinding> {
    inputs.iter().enumerate().map(|(idx, finding)| map_one(finding, idx + 1)).collect()
}

fn map_one(input: &Finding, index: usize) -> LintFinding {
    let rule_id = core_id_for(input.rule_id);
    let title = build_title(input.rule_id, &input.message, rule_id.is_some());
    let evidence = build_evidence(&input.message);
    let location = input.location.as_ref().map(map_location);

    let mut review = LintFinding {
        id: format!("FIND-{index:04}"),
        rule_id: rule_id.map(str::to_string),
        related_rule_ids: None,
        title,
        severity: severity_for(input.rule_id),
        source: FindingSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter: None,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Unknown,
        location,
        evidence,
        impact: format!("Authoring check '{}' failed.", input.rule_id),
        remediation: format!(
            "Resolve the violation reported by '{}'. See the finding message for details.",
            input.rule_id,
        ),
        confidence: None,
        fingerprint: String::new(),
        status: None,
        disposition: None,
    };
    review.fingerprint = fingerprint(&review);
    review
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

fn map_location(location: &Location) -> FindingLocation {
    let raw = location.path.to_string_lossy().into_owned();
    let normalised = raw.replace('\\', "/");
    FindingLocation {
        path: normalised,
        line: Some(usize_to_u32(location.line)),
        column: location.column.map(usize_to_u32),
        end_line: None,
        end_column: None,
    }
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
    use std::path::PathBuf;

    use specify_authoring::finding::{Finding, Location};
    use specify_lints::fingerprint::verify_fingerprint;
    use specify_lints::{Artifact, FindingEvidence, FindingSource, Severity, validate_finding};

    use super::{map_finding, map_findings};

    fn fixture(
        rule_id: &'static str, message: &str, path: Option<&str>, line: usize,
        column: Option<usize>,
    ) -> Finding {
        Finding {
            rule_id,
            message: message.to_owned(),
            location: path.map(|p| Location {
                path: PathBuf::from(p),
                line,
                column,
            }),
        }
    }

    /// (1) The mapper produces JSON-schema-valid output, including a
    /// recomputable fingerprint — the strongest correctness signal
    /// for the binary-boundary mapper.
    #[test]
    fn mapper_output_validates_schema() {
        let input = fixture(
            "rules.schema-violation",
            "Rule frontmatter failed schema validation.",
            Some("adapters/shared/rules/universal/example.md"),
            12,
            Some(4),
        );
        let mapped = map_finding(&input);
        validate_finding(&mapped).expect("mapped finding must validate against the schema");
    }

    /// (2) Severity wire-up: critical rule maps to Critical, ordinary
    /// rules map to Important via CH-20's table.
    #[test]
    fn severity_table_wires_through() {
        let critical = map_finding(&fixture("rules.schema-violation", "boom", None, 1, None));
        assert_eq!(critical.severity, Severity::Critical);

        let important = map_finding(&fixture("skill.unknown-tool", "dup", None, 1, None));
        assert_eq!(important.severity, Severity::Important);
    }

    /// (3) A mapped imperative id carries the closed codex `rule_id` on
    /// its own field, so the title stays clean (no `[...]` prefix) and
    /// collapses to the first non-empty message line.
    #[test]
    fn mapped_id_keeps_title_clean() {
        let mapped = map_finding(&fixture(
            "rules.schema-violation",
            "Rule frontmatter failed schema validation.\nsecond line ignored",
            None,
            1,
            None,
        ));
        assert_eq!(mapped.rule_id.as_deref(), Some("CORE-027"));
        assert_eq!(mapped.title, "Rule frontmatter failed schema validation.");
        assert!(!mapped.title.starts_with('['), "mapped id must not carry a title prefix");
    }

    /// (3b) An unmapped imperative id falls back to `rule_id: None` and
    /// the legacy `[...]` title prefix so a newly-added predicate is
    /// never silently dropped from the wire.
    #[test]
    fn unmapped_id_falls_back_to_title_prefix() {
        let mapped = map_finding(&fixture("future.unmapped-rule", "boom", None, 1, None));
        assert!(mapped.rule_id.is_none(), "unmapped id must yield rule_id: None");
        assert!(
            mapped.title.starts_with("[future.unmapped-rule] "),
            "unmapped id must lead with the authoring rule id: {}",
            mapped.title,
        );
    }

    /// (4) Each still-active imperative id maps onto a closed codex
    /// `CORE-NNN` id; `rules.namespace-ownership-violation` reuses its
    /// declarative counterpart `CORE-009`.
    #[test]
    fn rule_id_maps_to_core_namespace() {
        let cases = [
            ("rules.schema-violation", "CORE-027"),
            ("skill.unknown-tool", "CORE-047"),
            ("links.broken-reference", "CORE-019"),
            ("rules.namespace-ownership-violation", "CORE-009"),
        ];
        for (rule, core) in cases {
            let mapped = map_finding(&fixture(rule, "msg", None, 1, None));
            assert_eq!(mapped.rule_id.as_deref(), Some(core), "{rule} must map to {core}");
        }
    }

    /// (5) Authoring findings are produced by deterministic
    /// scanners, so `source` is always `Deterministic`.
    #[test]
    fn source_is_deterministic() {
        let mapped = map_finding(&fixture("skill.unknown-tool", "msg", None, 1, None));
        assert_eq!(mapped.source, FindingSource::Deterministic);
    }

    /// (6) Authoring findings are framework-internal — no slice,
    /// change, or adapter context — and carry `Artifact::Unknown`
    /// until a future enrichment pass classifies them.
    #[test]
    fn artifact_unknown_context_empty() {
        let mapped = map_finding(&fixture("skill.unknown-tool", "msg", None, 1, None));
        assert_eq!(mapped.artifact, Artifact::Unknown);
        assert!(mapped.slice.is_none());
        assert!(mapped.change.is_none());
        assert!(mapped.target_adapter.is_none());
        assert!(mapped.source_adapter.is_none());
        assert!(mapped.confidence.is_none());
        assert!(mapped.status.is_none());
    }

    /// (7) Location wiring: path forwarded verbatim (with separator
    /// normalisation), `usize` line/column widened to `u32`,
    /// `end_line` / `end_column` always `None` because the authoring
    /// `Location` does not carry ranges.
    #[test]
    fn location_widens_and_clears_endings() {
        let mapped =
            map_finding(&fixture("skill.unknown-tool", "msg", Some("foo/bar.md"), 42, Some(7)));
        let loc = mapped.location.expect("location must round-trip");
        assert_eq!(loc.path, "foo/bar.md");
        assert_eq!(loc.line, Some(42));
        assert_eq!(loc.column, Some(7));
        assert_eq!(loc.end_line, None);
        assert_eq!(loc.end_column, None);
    }

    /// (8) Batch mapping assigns sequential `FIND-{NNNN}` ids,
    /// 1-based and 4-digit zero-padded.
    #[test]
    fn map_findings_assigns_sequential_ids() {
        let inputs = vec![
            fixture("skill.unknown-tool", "a", None, 1, None),
            fixture("skill.unknown-tool", "b", None, 1, None),
            fixture("skill.unknown-tool", "c", None, 1, None),
        ];
        let mapped = map_findings(&inputs);
        let ids: Vec<&str> = mapped.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["FIND-0001", "FIND-0002", "FIND-0003"]);
    }

    /// (9) Identical inputs produce identical fingerprints — the
    /// underlying CH-15 algorithm is deterministic and the mapper
    /// must not introduce non-determinism (e.g. wall-clock,
    /// hash-map iteration order).
    #[test]
    fn fingerprint_deterministic() {
        let input = fixture(
            "skill.unknown-tool",
            "unknown `allowed-tools` entry in skill frontmatter",
            Some("plugins/spec/skills/build/SKILL.md"),
            3,
            Some(1),
        );
        let a = map_finding(&input);
        let b = map_finding(&input);
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    /// (10) The stored fingerprint matches the recomputed canonical
    /// fingerprint — proves the mapper assigns the field correctly
    /// and CH-15's `verify_fingerprint` short-circuits cleanly.
    #[test]
    fn stored_fingerprint_verifies() {
        let mapped = map_finding(&fixture(
            "links.broken-reference",
            "broken markdown reference",
            Some("docs/intro.md"),
            10,
            None,
        ));
        assert!(verify_fingerprint(&mapped));
    }

    /// (11) Oversize authoring messages spill into the `Digest`
    /// evidence variant so the serialised evidence stays under the
    /// 16 `KiB` cap and the finding still validates against the
    /// schema.
    #[test]
    fn oversize_message_becomes_digest_evidence() {
        let big_message = "a".repeat(17 * 1024);
        let mapped = map_finding(&fixture(
            "skill.unknown-tool",
            &big_message,
            Some("plugins/spec/skills/build/SKILL.md"),
            1,
            None,
        ));
        match &mapped.evidence {
            FindingEvidence::Digest { sha256, .. } => {
                assert_eq!(sha256.len(), 64, "digest must carry full sha256 hex");
            }
            other => panic!("expected Digest evidence for oversize message, got {other:?}"),
        }
        validate_finding(&mapped).expect("digest fallback must validate against the schema");
    }

    /// (12) Best-effort path-separator normalisation: forward slashes
    /// stay forward slashes, and any back-slash in the raw input
    /// (whether produced on Windows or hand-built in a test) is
    /// rewritten to `/`.
    #[test]
    fn path_separators_normalised() {
        let mapped =
            map_finding(&fixture("skill.unknown-tool", "msg", Some("foo\\bar\\baz.md"), 1, None));
        let loc = mapped.location.expect("location must round-trip");
        assert_eq!(loc.path, "foo/bar/baz.md", "back-slashes must be rewritten to forward slashes");
    }
}
