//! CH-21: map a `specify_authoring::Finding` to a structured RFC-28
//! [`ReviewFinding`].
//!
//! This module deliberately lives at the binary boundary
//! (`src/authoring/map_finding.rs`) alongside CH-20's
//! [`crate::authoring::severity`]. Per RFC-28 §"Relationship to
//! framework authoring", the `specify-authoring` library MUST NOT
//! depend on `specify-domain`; the structured-finding mapper therefore
//! has to live in the binary crate so the `specdev` JSON export can
//! reach across both worlds without polluting the authoring layer's
//! dependency graph.
//!
//! ## Mapping table
//!
//! | `Finding` source                       | `ReviewFinding` field                  |
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
//!
//! ### Decision: imperative `rule_id` vs closed codex `rule-id`
//!
//! `crates/authoring/src/finding.rs` returns a static authoring
//! identifier such as `codex.schema-violation`, `skill.duplicate-name`,
//! or `links.unresolved`. The wire schema at
//! `schemas/review/finding.schema.json` constrains `rule-id` to the
//! closed codex regex
//! `^(UNI|SRC|FRAME|RUST|IFACE|SEC|OMNIA|VECTIS|ORG)-[0-9]{3}$`.
//! Setting `rule_id: Some("codex.schema-violation".into())` would
//! therefore fail schema validation.
//!
//! To preserve the schema's closed contract while keeping the
//! authoring rule id human-greppable in downstream consumers, the
//! mapper leaves `rule_id: None` and surfaces the authoring id as a
//! `[...]` prefix on `title`. The brackets are parseable so future
//! tooling can recover the imperative id without re-running the
//! check.
//!
//! TODO(RFC-32): once authoring rule families migrate to codex
//! `FRAME-NNN` ids (the declarative framework-rule namespace
//! introduced by RFC-32), this mapper should set
//! `rule_id: Some("FRAME-NNN")` and drop the `[...]` title prefix.

use specify_authoring::finding::{Finding, Location};
use specify_domain::codex::fingerprint::fingerprint;
use specify_domain::codex::{
    Artifact, FindingEvidence, FindingLocation, FindingSource, ReviewFinding,
};
use specify_tool::sha256_hex;

use crate::authoring::severity::severity_for;

/// 16 `KiB` cap on the serialised evidence object per RFC-28 (mirror
/// of `specify_domain::codex::finding::EVIDENCE_MAX_BYTES`, kept
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

/// Map a single authoring [`Finding`] to a [`ReviewFinding`] with id
/// `FIND-0001`.
///
/// Equivalent to `map_findings(&[input.clone()]).into_iter().next()`
/// but avoids the allocation. The fingerprint is computed last so
/// every other field is hashed exactly as serialised.
#[must_use]
pub fn map_finding(input: &Finding) -> ReviewFinding {
    map_one(input, 1)
}

/// Map a batch of authoring [`Finding`]s to [`ReviewFinding`]s,
/// assigning sequential `FIND-{NNNN}` ids in input order (1-based,
/// 4-digit zero-padded).
///
/// The sequence is producer-local: it MUST NOT be assumed stable
/// across runs because reordering in upstream `Check::run`
/// implementations will shuffle the ids. Callers that need a stable
/// dedup key SHOULD use [`ReviewFinding::fingerprint`] instead.
#[must_use]
pub fn map_findings(inputs: &[Finding]) -> Vec<ReviewFinding> {
    inputs.iter().enumerate().map(|(idx, finding)| map_one(finding, idx + 1)).collect()
}

fn map_one(input: &Finding, index: usize) -> ReviewFinding {
    let title = build_title(input.rule_id, &input.message);
    let evidence = build_evidence(&input.message);
    let location = input.location.as_ref().map(map_location);

    let mut review = ReviewFinding {
        id: format!("FIND-{index:04}"),
        rule_id: None,
        related_rule_ids: None,
        title,
        severity: severity_for(input.rule_id),
        source: FindingSource::Deterministic,
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
    };
    review.fingerprint = fingerprint(&review);
    review
}

fn build_title(rule_id: &str, message: &str) -> String {
    let head = message.lines().find(|line| !line.trim().is_empty()).unwrap_or(message);
    let head = head.trim();
    let body = if head.is_empty() { "(no message)" } else { head };
    let raw = format!("[{rule_id}] {body}");
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
    use specify_domain::codex::fingerprint::verify_fingerprint;
    use specify_domain::codex::{
        Artifact, FindingEvidence, FindingSource, Severity, validate_finding,
    };

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
    fn mapper_output_validates_against_review_finding_schema() {
        let input = fixture(
            "codex.schema-violation",
            "Codex rule frontmatter failed schema validation.",
            Some("adapters/shared/codex/universal/example.md"),
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
        let critical = map_finding(&fixture("codex.schema-violation", "boom", None, 1, None));
        assert_eq!(critical.severity, Severity::Critical);

        let important = map_finding(&fixture("skill.duplicate-name", "dup", None, 1, None));
        assert_eq!(important.severity, Severity::Important);
    }

    /// (3) The synthesised title carries the authoring rule id as a
    /// `[...]` prefix so downstream consumers can recover the
    /// imperative identifier even though `rule_id` is `None`.
    #[test]
    fn title_prefixes_authoring_rule_id_in_brackets() {
        let mapped = map_finding(&fixture(
            "codex.schema-violation",
            "Codex rule frontmatter failed schema validation.\nsecond line ignored",
            None,
            1,
            None,
        ));
        assert!(
            mapped.title.starts_with("[codex.schema-violation] "),
            "title must lead with the authoring rule id: {}",
            mapped.title,
        );
        assert!(
            !mapped.title.contains('\n'),
            "title must collapse to a single line: {:?}",
            mapped.title,
        );
    }

    /// (4) Authoring imperative ids (`codex.schema-violation`,
    /// `skill.duplicate-name`, ...) do not match the codex `rule-id`
    /// regex, so the mapper leaves `rule_id: None` and keeps the
    /// schema legal.
    #[test]
    fn rule_id_is_omitted_for_imperative_ids() {
        for rule in ["codex.schema-violation", "skill.duplicate-name", "links.unresolved"] {
            let mapped = map_finding(&fixture(rule, "msg", None, 1, None));
            assert!(mapped.rule_id.is_none(), "{rule} must yield rule_id: None");
        }
    }

    /// (5) Authoring findings are produced by deterministic
    /// scanners, so `source` is always `Deterministic`.
    #[test]
    fn source_is_deterministic() {
        let mapped = map_finding(&fixture("skill.duplicate-name", "msg", None, 1, None));
        assert_eq!(mapped.source, FindingSource::Deterministic);
    }

    /// (6) Authoring findings are framework-internal — no slice,
    /// change, or adapter context — and carry `Artifact::Unknown`
    /// until a future enrichment pass classifies them.
    #[test]
    fn artifact_is_unknown_and_context_is_empty() {
        let mapped = map_finding(&fixture("skill.duplicate-name", "msg", None, 1, None));
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
    fn location_widens_usize_fields_and_clears_range_endings() {
        let mapped =
            map_finding(&fixture("skill.duplicate-name", "msg", Some("foo/bar.md"), 42, Some(7)));
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
            fixture("skill.duplicate-name", "a", None, 1, None),
            fixture("skill.duplicate-name", "b", None, 1, None),
            fixture("skill.duplicate-name", "c", None, 1, None),
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
    fn fingerprint_is_deterministic_across_runs() {
        let input = fixture(
            "skill.duplicate-name",
            "duplicate `name:` field in skill frontmatter",
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
            "links.unresolved",
            "broken markdown link",
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
            "skill.duplicate-name",
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
    fn path_separators_are_normalised_to_forward_slash() {
        let mapped =
            map_finding(&fixture("skill.duplicate-name", "msg", Some("foo\\bar\\baz.md"), 1, None));
        let loc = mapped.location.expect("location must round-trip");
        assert_eq!(loc.path, "foo/bar/baz.md", "back-slashes must be rewritten to forward slashes");
    }
}
