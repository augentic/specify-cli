use super::*;
use crate::rules::{Origin, PathRoot};

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
        deterministic_hints: None,
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
