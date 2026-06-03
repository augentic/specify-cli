//! Finding constructors and the §"Evidence cap" truncation pass
//! shared by every hint sub-evaluator.

use specify_diagnostics::{
    Artifact, Confidence, Diagnostic, DiagnosticKind, DiagnosticSource, FindingEvidence,
    FindingLocation, Severity, fingerprint as compute_fingerprint, validate_evidence_size,
};

use super::ReservedSkipped;
use crate::rules::ResolvedRule;

/// Mint the reserved-hint diagnostics reserved-hint summary finding from accumulated
/// [`ReservedSkipped`] entries, or return `None` when the input is
/// empty.
///
/// `strict_hints` upgrades the finding's severity from `optional` to
/// `important`; the `rule_id` is `review.reserved-hint-skipped` in
/// both modes per reserved-hint diagnostics.
#[must_use]
pub fn reserved_hint_summary(
    skipped: &[ReservedSkipped], strict_hints: bool,
) -> Option<Diagnostic> {
    if skipped.is_empty() {
        return None;
    }
    let pairs: Vec<serde_json::Value> =
        skipped.iter().map(|s| serde_json::json!([s.rule_id, s.hint_index])).collect();
    let evidence = FindingEvidence::Structured {
        summary: "Reserved hint kinds awaiting implementation".to_string(),
        data: serde_json::json!({ "pairs": pairs }),
        locations: None,
    };
    let severity = if strict_hints { Severity::Important } else { Severity::Optional };
    let mut finding = Diagnostic {
        id: "FIND-RESERVED".to_string(),
        rule_id: Some("review.reserved-hint-skipped".to_string()),
        related_rule_ids: None,
        title: "Reserved hint kinds awaiting implementation".to_string(),
        severity,
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter: None,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Unknown,
        location: None,
        evidence,
        impact: format!("{} reserved hint occurrence(s) skipped this scan.", skipped.len()),
        remediation: "Implement the reserved hint kind or remove the hint until support lands."
            .to_string(),
        confidence: Some(Confidence::High),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    };
    clamp_evidence(&mut finding);
    finding.fingerprint = compute_fingerprint(&finding);
    Some(finding)
}

/// Apply the §"Evidence cap" truncation and stamp the structured lint
/// finding fingerprint. Clamp BEFORE signing. Shared by every
/// finding builder so the stamp can never be forgotten.
fn finalize(mut finding: Diagnostic) -> Diagnostic {
    clamp_evidence(&mut finding);
    finding.fingerprint = compute_fingerprint(&finding);
    finding
}

/// Build a finding from rule-derived defaults (severity, target
/// adapter, impact, remediation), apply the §"Evidence cap"
/// truncation, and stamp the structured lint finding fingerprint.
pub fn make_finding(
    rule: &ResolvedRule, id_num: u64, title: String, location: Option<FindingLocation>,
    evidence: FindingEvidence,
) -> Diagnostic {
    finalize(Diagnostic {
        id: format!("FIND-{id_num:04}"),
        rule_id: Some(rule.rule_id.clone()),
        related_rule_ids: None,
        title,
        severity: rule.severity,
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter: single_adapter(rule),
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location,
        evidence,
        impact: rule.trigger.clone(),
        remediation: format!("See {}", rule.path),
        confidence: Some(Confidence::High),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    })
}

/// Build a non-blocking `kind: review` diagnostic for a
/// `lint-mode: model-assisted` rule the deterministic engine cannot
/// score. The rule's `trigger` becomes the review prompt (impact +
/// snippet evidence) and its `path` the remediation pointer. Source is
/// `model-assisted` — the question is destined for a scorer, not a
/// deterministic verdict.
pub(super) fn make_review_finding(rule: &ResolvedRule, id_num: u64) -> Diagnostic {
    finalize(Diagnostic {
        id: format!("FIND-{id_num:04}"),
        rule_id: Some(rule.rule_id.clone()),
        related_rule_ids: None,
        title: rule.title.clone(),
        severity: rule.severity,
        source: DiagnosticSource::ModelAssisted,
        kind: DiagnosticKind::Review,
        target_adapter: single_adapter(rule),
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location: None,
        evidence: FindingEvidence::Snippet {
            value: rule.trigger.clone(),
        },
        impact: rule.trigger.clone(),
        remediation: format!("Model-assisted review required; see {}", rule.path),
        confidence: Some(Confidence::Medium),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    })
}

/// Inputs for [`make_synthetic_finding`].
///
/// Named fields keep the synthetic-finding call sites readable: the
/// `tool.undeclared` / `tool.invocation-failed` shapes pass several
/// optional values (`location`, `target_adapter`) that would otherwise
/// be bare positional `None`s.
pub struct SyntheticFinding<'a> {
    /// Monotonic finding number rendered into the `FIND-NNNN` id.
    pub id_num: u64,
    /// Explicit rule id stamped on the finding.
    pub rule_id: &'a str,
    /// Human-readable finding title.
    pub title: String,
    /// Finding severity.
    pub severity: Severity,
    /// Optional source location.
    pub location: Option<FindingLocation>,
    /// Structured evidence payload.
    pub evidence: FindingEvidence,
    /// Impact line.
    pub impact: String,
    /// Remediation line.
    pub remediation: String,
    /// Optional owning target adapter.
    pub target_adapter: Option<String>,
}

/// Build a finding with an explicit `rule_id` / `severity` (for the
/// synthetic `tool.undeclared` and `tool.invocation-failed` shapes).
pub fn make_synthetic_finding(spec: SyntheticFinding<'_>) -> Diagnostic {
    let SyntheticFinding {
        id_num,
        rule_id,
        title,
        severity,
        location,
        evidence,
        impact,
        remediation,
        target_adapter,
    } = spec;
    finalize(Diagnostic {
        id: format!("FIND-{id_num:04}"),
        rule_id: Some(rule_id.to_string()),
        related_rule_ids: None,
        title,
        severity,
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location,
        evidence,
        impact,
        remediation,
        confidence: Some(Confidence::High),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    })
}

/// Stamp `id` and recompute the fingerprint on a finding produced
/// outside the rule-derived defaults (e.g. forwarded from a tool's
/// stdout). Applies the evidence-cap truncation before signing.
pub fn restamp_finding(finding: &mut Diagnostic, id_num: u64) {
    finding.id = format!("FIND-{id_num:04}");
    clamp_evidence(finding);
    finding.fingerprint = compute_fingerprint(finding);
}

fn single_adapter(rule: &ResolvedRule) -> Option<String> {
    let adapters = rule.applicability.as_ref().and_then(|a| a.adapters.as_ref())?;
    if adapters.len() != 1 {
        return None;
    }
    let raw = adapters[0].as_str();
    Some(raw.split_once('@').map_or_else(|| raw.to_owned(), |(name, _)| name.to_owned()))
}

const TRUNCATION_MARKER: &str = "…[truncated]";
const CLAMP_ITERATION_LIMIT: usize = 32;

fn clamp_evidence(finding: &mut Diagnostic) {
    let mut iter = 0;
    while validate_evidence_size(finding).is_err() && iter < CLAMP_ITERATION_LIMIT {
        iter += 1;
        match &mut finding.evidence {
            FindingEvidence::Snippet { value } => {
                if value.is_empty() {
                    break;
                }
                let target = value.len() / 2;
                let mut cut = target;
                while cut > 0 && !value.is_char_boundary(cut) {
                    cut -= 1;
                }
                value.truncate(cut);
                value.push_str(TRUNCATION_MARKER);
            }
            FindingEvidence::Structured { data, locations, .. } => {
                *data = serde_json::json!({ "truncated": true });
                *locations = None;
            }
            FindingEvidence::Digest { .. } => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use specify_diagnostics::{
        Artifact, Confidence, Diagnostic, DiagnosticKind, DiagnosticSource, FindingEvidence,
        Severity, validate_evidence_size,
    };

    use super::{TRUNCATION_MARKER, clamp_evidence, reserved_hint_summary, single_adapter};
    use crate::lint::eval::ReservedSkipped;
    use crate::rules::{HintKind, Origin, PathRoot, ResolvedRule};

    fn rule(adapters: Option<Vec<String>>) -> ResolvedRule {
        ResolvedRule {
            rule_id: "UNI-001".into(),
            title: "t".into(),
            severity: Severity::Important,
            trigger: "trigger".into(),
            lint_mode: None,
            applicability: adapters.map(|a| crate::rules::Applicability {
                adapters: Some(a),
                languages: None,
                artifacts: None,
                paths: None,
            }),
            rule_hints: None,
            references: None,
            origin: Origin::Shared,
            path_root: PathRoot::RulesRoot,
            path: "shared/UNI-001.md".into(),
            body: String::new(),
            deprecated: None,
        }
    }

    #[test]
    fn single_adapter_strips_version_suffix() {
        let r = rule(Some(vec!["omnia@v2".into()]));
        assert_eq!(single_adapter(&r).as_deref(), Some("omnia"));
    }

    #[test]
    fn single_adapter_none_when_multiple() {
        let r = rule(Some(vec!["omnia".into(), "vectis".into()]));
        assert!(single_adapter(&r).is_none());
    }

    #[test]
    fn reserved_summary_empty_when_none() {
        assert!(reserved_hint_summary(&[], false).is_none());
    }

    #[test]
    fn reserved_summary_stable_rule_id() {
        let skipped = vec![ReservedSkipped {
            rule_id: "UNI-099".into(),
            hint_index: 0,
            kind: HintKind::NamespaceOwner,
        }];
        let optional = reserved_hint_summary(&skipped, false).expect("present");
        let strict = reserved_hint_summary(&skipped, true).expect("present");
        assert_eq!(optional.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
        assert_eq!(strict.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
        assert_eq!(optional.severity, Severity::Optional);
        assert_eq!(strict.severity, Severity::Important);
    }

    #[test]
    fn clamp_truncates_oversize_snippet() {
        let mut finding = Diagnostic {
            id: "FIND-0001".into(),
            rule_id: Some("UNI-001".into()),
            related_rule_ids: None,
            title: "t".into(),
            severity: Severity::Important,
            source: DiagnosticSource::Deterministic,
            kind: DiagnosticKind::Violation,
            target_adapter: None,
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Code,
            location: None,
            evidence: FindingEvidence::Snippet {
                value: "a".repeat(64 * 1024),
            },
            impact: "i".into(),
            remediation: "r".into(),
            confidence: Some(Confidence::High),
            fingerprint: String::new(),
            status: None,
            disposition: None,
        };
        clamp_evidence(&mut finding);
        validate_evidence_size(&finding).expect("evidence fits within cap");
        if let FindingEvidence::Snippet { value } = &finding.evidence {
            assert!(value.ends_with(TRUNCATION_MARKER));
        } else {
            panic!("snippet variant preserved");
        }
    }
}
