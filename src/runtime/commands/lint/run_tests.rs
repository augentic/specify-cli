use std::path::PathBuf;

use specify_codex::HintKind;

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
        origin: specify_codex::Origin::Shared,
        path_root: specify_codex::PathRoot::CodexRoot,
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
];

const HINT_VALIDATION_CASES: &[ValidationCase<HintError>] = &[
    ValidationCase {
        err: || HintError::Unsupported {
            rule_id: RULE_ID.into(),
            kind: HintKind::Unique,
            reason: "reserved",
        },
        rule_id: "review-unsupported-hint-kind",
    },
    ValidationCase {
        err: || HintError::SchemaCompile {
            rule_id: RULE_ID.into(),
            schema_ref: "codex-rule".into(),
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
        other => panic!("§D8: expected Validation({rule_id}), got {other:?}"),
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
        other => panic!("§D8: expected Filesystem, got {other:?}"),
    }
    for case in RENDER_CASES {
        match map_render_error((case.err)()) {
            Error::Diag { code, .. } => assert_eq!(code, case.code),
            other => panic!("§D8: expected Diag({}), got {other:?}", case.code),
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
