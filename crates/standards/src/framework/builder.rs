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

use std::path::{Path, PathBuf};

use specify_diagnostics::{
    Artifact, Diagnostic, DiagnosticKind, DiagnosticSource, FindingEvidence, FindingLocation,
    Severity,
};
use specify_digest::sha256_hex;

use crate::framework::error::ToolingError;

/// Mapping from each still-active imperative authoring rule id to its
/// closed codex `CORE-NNN` id.
///
/// After RFC-31 Phase 4 only the CORE-009 namespace bridge remains
/// imperative. `CORE-001..008` and `CORE-010..052` run through
/// declarative `CORE-*` rule files (`kind: authoring-predicate` or
/// native hints). See `adapters/shared/rules/core/` in augentic/specify.
const CORE_ID_TABLE: &[(&str, &str)] = &[("rules.namespace-ownership-violation", "CORE-009")];

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
/// file breaks every downstream consumer of the resolved codex. Every
/// other authoring rule maps to the `Important` default.
#[must_use]
pub fn severity_for(rule_id: &str) -> Severity {
    match rule_id {
        "rules.schema-violation" => Severity::Critical,
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

/// Build a finding for an infrastructure failure (a [`ToolingError`]
/// raised while a predicate walks the tree), with no location.
#[must_use]
pub fn infrastructure_finding(rule_id: &'static str, error: ToolingError) -> Diagnostic {
    framework_finding(rule_id, error.to_string(), None)
}

/// Build a finding anchored at line 1 of an optional path. `None`
/// yields a location-less finding.
#[must_use]
pub fn finding(rule_id: &'static str, message: String, path: Option<PathBuf>) -> Diagnostic {
    framework_finding(rule_id, message, path.map(|path| loc(path, 1, None)))
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
mod tests;
