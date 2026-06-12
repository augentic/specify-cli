//! End-to-end behavior edges for `specify lint framework` that the JSON
//! goldens in `framework_json.rs` do not pin:
//!
//! - Framework self-lint writes **no** journal: the `lint-completed`
//!   contract is scoped to `specify lint project` (DECISIONS.md
//!   §"Journal event names").
//! - The retired `kind: authoring-predicate` bridge no longer parses.
//! - A duplicate rule id pre-empts the whole declarative pass.
//!
//! Envelope shape, finding contents, and the human formatter are
//! covered by the goldens + text smoke in `framework_json.rs`; the
//! per-kind evaluator semantics live as crate-level unit tests in
//! `specify-standards`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use specify_standards::rules::{HintKind, ParseError, parse_rule};
use tempfile::TempDir;

use crate::support::scaffold_framework;

/// Run `specify lint framework --framework-root <root> --output-format json`
/// and return the captured `(exit, stdout, stderr)` triple.
fn run_lint_framework(root: &Path, args: &[&str]) -> (Option<i32>, Vec<u8>, Vec<u8>) {
    let output = Command::cargo_bin("specify")
        .expect("cargo_bin(specify)")
        .args(["lint", "framework", "--framework-root"])
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("specify lint framework invocation");
    (output.status.code(), output.stdout, output.stderr)
}

/// Framework self-lint writes no journal. The `lint-completed` contract
/// is scoped to `specify lint project` (DECISIONS.md §"Journal event
/// names"), so a `specify lint framework` run must not create
/// `<framework_root>/.specify/journal.jsonl`.
#[test]
fn framework_lint_writes_no_journal() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());

    let journal_path = temp.path().join(".specify").join("journal.jsonl");
    assert!(!journal_path.exists(), "precondition: journal must not exist before the run");

    let (_code, _stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "json"]);

    assert!(
        !journal_path.exists(),
        "framework self-lint must not journal, but found {}; stderr:\n{}",
        journal_path.display(),
        String::from_utf8_lossy(&stderr),
    );
}

/// Write two data rule files that share the id `CORE-100`, which the
/// `rules.duplicate-rule-id` predicate flags as a whole-tree duplicate.
/// Both are otherwise schema-valid and carry no hints, so the only
/// finding the predicate produces is the duplicate-id collision.
fn write_duplicate_rule_id(root: &Path) {
    let core_dir = root.join("adapters/shared/rules/core");
    fs::create_dir_all(&core_dir).expect("mkdir core rules");
    for file in ["CORE-100-first.md", "CORE-100-second.md"] {
        fs::write(
            core_dir.join(file),
            "---\n\
id: CORE-100\n\
title: Synthetic Duplicate\n\
severity: important\n\
trigger: A synthetic rule used to exercise duplicate-id detection.\n\
---\n\n\
## Rule\n\n\
Synthetic data rule sharing an id with its sibling.\n",
        )
        .expect("write duplicate rule");
    }
}

/// Parse the framework run's stdout envelope, panicking with stderr
/// context on a non-JSON body.
fn envelope(stdout: &[u8], stderr: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|err| {
        panic!("stdout is not JSON: {err}; stderr:\n{}", String::from_utf8_lossy(stderr))
    })
}

/// Post-bridge invariant: the `kind: authoring-predicate` mechanism is
/// gone. Rule-agnostic — it pins the
/// *mechanism*, not any `CORE-NNN`: the closed `HintKind` enum no longer
/// carries the bridge discriminant, and a rule file that still declares
/// it fails `rule.schema.json` validation rather than dispatching to an
/// in-engine imperative predicate. The framework lint therefore resolves
/// every rule through declarative hints + referenced tools only.
#[test]
fn authoring_predicate_kind_is_removed() {
    assert!(
        serde_json::from_value::<HintKind>(Value::String("authoring-predicate".into())).is_err(),
        "HintKind must no longer carry the authoring-predicate bridge variant",
    );

    let rule = "---\n\
id: CORE-999\n\
title: Retired Bridge Kind\n\
severity: important\n\
trigger: A rule that still declares the removed authoring-predicate bridge kind.\n\
rule_hints:\n\
\x20 - kind: authoring-predicate\n\
\x20   value: scenarios.stale-recorded-trace\n\
---\n\n\
## Rule\n\n\
The authoring-predicate bridge has been removed.\n";
    let err = parse_rule(rule).expect_err("the retired bridge kind must no longer parse");
    assert!(
        matches!(err, ParseError::Schema(_)),
        "expected a rule-schema rejection of the retired kind, got: {err:?}",
    );
}

/// The engine's resolver-level duplicate-id guard skips the entire
/// declarative pass when two rule files share an id: every declarative
/// rule (including the `rules` checker that owns CORE-026) is
/// pre-empted, so no `rules.*` finding can surface through the binary on a
/// duplicate-id tree. The run still completes (exit 0) with the skip
/// signalled on stderr.
#[test]
fn duplicate_rule_id_skips_declarative_pass() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_duplicate_rule_id(temp.path());

    let (code, stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "json"]);
    let envelope = envelope(&stdout, &stderr);
    let findings = envelope.get("findings").and_then(Value::as_array).expect("findings array");
    assert!(
        !findings.iter().any(|f| f.get("rule-id").and_then(Value::as_str) == Some("CORE-026")),
        "the resolver guard pre-empts the declarative pass, so CORE-026 never fires; got envelope:\n{envelope:#}",
    );
    let stderr_text = String::from_utf8_lossy(&stderr);
    assert!(
        stderr_text.contains("declarative pass skipped"),
        "a duplicate rule id must skip the declarative pass; stderr:\n{stderr_text}",
    );
    assert_eq!(code, Some(0), "a skipped declarative pass still completes");
}
