use super::Diagnostic;

/// REVIEW.md A18: a branch-mutation [`Diagnostic`] projects onto the
/// canonical diagnostic currency as a deterministic `Important`
/// violation, with the `key` as `rule_id`, the registry `project` as
/// `change`, and a fingerprint that validates.
#[test]
fn branch_diagnostic_projects_onto_canonical_diagnostic() {
    let branch = Diagnostic {
        key: "dirty-tracked".to_string(),
        project: "monolith".to_string(),
        message: "tracked changes present outside allowed paths".to_string(),
        branch: Some("slice/checkout".to_string()),
        paths: vec!["src/main.rs".to_string()],
    };
    let diagnostic = specify_diagnostics::Diagnostic::from(&branch);

    assert_eq!(diagnostic.rule_id.as_deref(), Some("dirty-tracked"));
    assert_eq!(diagnostic.severity, specify_diagnostics::Severity::Important);
    assert_eq!(diagnostic.kind, specify_diagnostics::DiagnosticKind::Violation);
    assert_eq!(diagnostic.change.as_deref(), Some("monolith"));
    specify_diagnostics::validate_diagnostic(&diagnostic).expect("projected diagnostic is valid");
    assert!(specify_diagnostics::verify_fingerprint(&diagnostic), "fingerprint covers change");
}
