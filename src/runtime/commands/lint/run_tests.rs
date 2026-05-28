use std::path::PathBuf;

use specify_lints::{
    Artifact, Confidence, FindingEvidence, FindingLocation, FindingSource, FindingStatus, HintKind,
    Severity,
};

use super::*;

fn fake_rule() -> ResolvedRule {
    ResolvedRule {
        rule_id: "UNI-001".into(),
        title: "t".into(),
        severity: Severity::Important,
        trigger: "trigger".into(),
        lint_mode: None,
        applicability: None,
        deterministic_hints: None,
        references: None,
        origin: specify_lints::Origin::Shared,
        path_root: specify_lints::PathRoot::RulesRoot,
        path: "shared/UNI-001.md".into(),
        body: String::new(),
        deprecated: None,
    }
}

const RULE_ID: &str = "UNI-001";

struct ValidationCase<E> {
    err: fn() -> E,
    rule_id: &'static str,
}

struct DiagCase {
    err: fn() -> RenderError,
    code: &'static str,
}

fn hint_regex_compile() -> HintError {
    let pattern = "(".to_string();
    let source = ::regex::Regex::new(&pattern).expect_err("invalid regex");
    HintError::RegexCompile {
        rule_id: RULE_ID.into(),
        pattern,
        source,
    }
}

const INDEX_CASES: &[ValidationCase<IndexError>] = &[
    ValidationCase {
        err: || IndexError::UnsupportedScanProfile(ScanProfile::Framework),
        rule_id: "review-unsupported-scan-profile",
    },
    ValidationCase {
        err: || IndexError::ProjectDirMissing(PathBuf::from("/missing")),
        rule_id: "review-project-dir-missing",
    },
    ValidationCase {
        err: || IndexError::OverrideCompile("bad glob".into()),
        rule_id: "review-index-override-compile",
    },
    ValidationCase {
        err: || IndexError::Filesystem("symlink cycle at <link>".into()),
        rule_id: "review-index-filesystem",
    },
];

const HINT_VALIDATION_CASES: &[ValidationCase<HintError>] = &[
    ValidationCase {
        err: || HintError::Unsupported {
            rule_id: RULE_ID.into(),
            kind: HintKind::SetCoverage,
            reason: "reserved",
        },
        rule_id: "review-unsupported-hint-kind",
    },
    ValidationCase {
        err: || HintError::SchemaCompile {
            rule_id: RULE_ID.into(),
            schema_ref: "rule".into(),
            detail: "compile failed".into(),
        },
        rule_id: "review-schema-compile-failed",
    },
    ValidationCase {
        err: || HintError::SchemaResolve {
            rule_id: RULE_ID.into(),
            schema_ref: "missing".into(),
            reason: "no such id".into(),
        },
        rule_id: "review-schema-resolve-failed",
    },
    ValidationCase {
        err: hint_regex_compile,
        rule_id: "review-regex-compile-failed",
    },
    ValidationCase {
        err: || HintError::ToolInvocation {
            rule_id: RULE_ID.into(),
            tool: "contract".into(),
            detail: "runtime".into(),
        },
        rule_id: "review-tool-invocation-failed",
    },
    ValidationCase {
        err: || HintError::ToolUndeclared {
            rule_id: RULE_ID.into(),
            tool: "contract".into(),
        },
        rule_id: "review-tool-undeclared",
    },
];

const RENDER_CASES: &[DiagCase] = &[
    DiagCase {
        err: || RenderError::JsonSchemaValidation {
            detail: "schema mismatch".into(),
        },
        code: "review-envelope-schema",
    },
    DiagCase {
        err: || {
            RenderError::JsonSerialise(
                serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
            )
        },
        code: "review-envelope-serialise",
    },
];

fn assert_validation_rule_id(got: Error, rule_id: &str) {
    match got {
        Error::Validation { results } => assert_eq!(results[0].rule_id, rule_id),
        other => panic!("lint exit mapping: expected Validation({rule_id}), got {other:?}"),
    }
}

#[test]
fn error_mapping_matches_d8_table() {
    let rule = fake_rule();
    for case in INDEX_CASES {
        assert_validation_rule_id(map_index_error((case.err)()), case.rule_id);
    }
    for case in HINT_VALIDATION_CASES {
        assert_validation_rule_id(map_hint_error(&rule, (case.err)()), case.rule_id);
    }
    match map_hint_error(
        &rule,
        HintError::Filesystem {
            op: "read",
            path: PathBuf::from("/missing"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        },
    ) {
        Error::Filesystem { op, path, .. } => {
            assert_eq!(op, "review-eval");
            assert_eq!(path, PathBuf::from("/missing"));
        }
        other => panic!("lint exit mapping: expected Filesystem, got {other:?}"),
    }
    for case in RENDER_CASES {
        match map_render_error((case.err)()) {
            Error::Diag { code, .. } => assert_eq!(code, case.code),
            other => panic!("lint exit mapping: expected Diag({}), got {other:?}", case.code),
        }
    }
}

#[test]
fn slice_tasks_parser_collects_bullet_paths() {
    let text = "## Tasks\n\n- intro\n\n## Touches\n\n- crates/billing/src/lib.rs\n* docs/billing.md\n\n## Notes\n\n- unrelated\n";
    let paths = parse_slice_tasks_paths(text);
    assert_eq!(
        paths,
        vec![PathBuf::from("crates/billing/src/lib.rs"), PathBuf::from("docs/billing.md"),]
    );
}

#[test]
fn slice_tasks_parser_handles_both_touches_and_produces() {
    let text = "## Produces\n\n- a.md\n\n## Touches\n\n- b.md\n";
    let paths = parse_slice_tasks_paths(text);
    assert_eq!(paths, vec![PathBuf::from("a.md"), PathBuf::from("b.md")]);
}

fn exit_fixture_finding(severity: Severity, status: Option<FindingStatus>) -> LintFinding {
    LintFinding {
        id: "FIND-0001".into(),
        rule_id: Some("UNI-014".into()),
        related_rule_ids: None,
        title: "exit-test finding".into(),
        severity,
        source: FindingSource::Deterministic,
        target_adapter: None,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location: Some(FindingLocation {
            path: "src/lib.rs".into(),
            line: Some(1),
            column: None,
            end_line: None,
            end_column: None,
        }),
        evidence: FindingEvidence::Snippet { value: "x".into() },
        impact: "i".into(),
        remediation: "r".into(),
        confidence: Some(Confidence::High),
        fingerprint: format!("sha256:{}", "0".repeat(64)),
        status,
        disposition: None,
    }
}

fn exit_result(findings: Vec<LintFinding>) -> LintResult {
    LintResult {
        version: LintResultVersion,
        summary: LintSummary::from_findings(&findings),
        findings,
    }
}

/// RFC-33a §"Exit and presentation semantics": exit 2 fires only
/// when at least one finding carries `status: open` AND severity is
/// critical or important. Ignored / false-positive findings stay in
/// the envelope but do not block.
#[test]
fn decide_exit_is_status_aware() {
    // Empty envelope → exit 0.
    decide_exit(&exit_result(vec![])).expect("empty envelope must exit 0");

    // Critical but ignored → exit 0.
    let critical_ignored = exit_fixture_finding(Severity::Critical, Some(FindingStatus::Ignored));
    decide_exit(&exit_result(vec![critical_ignored.clone()]))
        .expect("ignored critical must not block");

    // Important but false-positive → exit 0.
    let important_fp =
        exit_fixture_finding(Severity::Important, Some(FindingStatus::FalsePositive));
    decide_exit(&exit_result(vec![important_fp.clone()]))
        .expect("false-positive important must not block");

    // Suggestion + Open → exit 0 (severity below the blocking
    // threshold).
    let open_suggestion = exit_fixture_finding(Severity::Suggestion, Some(FindingStatus::Open));
    decide_exit(&exit_result(vec![open_suggestion])).expect("suggestion severity must not block");

    // Critical + Open → exit 2.
    let critical_open = exit_fixture_finding(Severity::Critical, Some(FindingStatus::Open));
    let err = decide_exit(&exit_result(vec![critical_open])).expect_err("open critical blocks");
    match err {
        Error::Validation { results } => {
            assert_eq!(results[0].rule_id, "review-findings-present");
        }
        other => panic!("expected Validation, got {other:?}"),
    }

    // Unset status + Important → exit 2 (raw scanner output).
    let important_unset = exit_fixture_finding(Severity::Important, None);
    let err =
        decide_exit(&exit_result(vec![important_unset])).expect_err("unset status treated as open");
    assert!(matches!(err, Error::Validation { .. }));

    // Mixed: one ignored critical + one open important → blocks.
    let mixed = vec![
        critical_ignored,
        important_fp,
        exit_fixture_finding(Severity::Important, Some(FindingStatus::Open)),
    ];
    let err = decide_exit(&exit_result(mixed)).expect_err("any open critical/important blocks");
    assert!(matches!(err, Error::Validation { .. }));
}
