//! End-to-end test for the `specify lint framework` extension.
//!
//! Exercises the binary surface added for framework convergence:
//!
//! - `specify lint framework --output-format json` against a small
//!   framework-shaped scaffold emits a `DiagnosticReport` envelope on
//!   stdout that validates against
//!   `schemas/diagnostics/diagnostic-report.schema.json`.
//! - The same run lands exactly one `lint-completed` journal event
//!   in `<framework-root>/.specify/journal.jsonl`,
//!   with the mandated `scope.target = null`,
//!   `scope.slice = null`, `baseline_present = false` shape.
//! - The pretty formatter (`--output-format pretty`) produces a
//!   non-empty human summary, confirming the four-formatter set
//!   from [`specify_diagnostics`] round-trips through
//!   the new verb.
//!
//! The scaffold mirrors `tests/lint_framework_json.rs::write_scaffold`
//! and is deliberately small: just enough framework structure to
//! satisfy `Context::is_framework_root` and silence the marketplace
//! / agent-teams predicates so the JSON envelope shape — not
//! individual finding contents — is what this test pins.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use specify_standards::rules::{HintKind, ParseError, parse_rule};
use tempfile::TempDir;

/// Scaffold a minimal framework tree that passes
/// `specify_standards::framework::context::Context::from_framework_root` and
/// supplies the marketplace + canonical-doc files the framework rules'
/// referenced tools expect. Intentionally identical in shape to
/// the scaffold used by `tests/lint_framework_json.rs` so both
/// surfaces exercise the same fixture profile.
fn scaffold_framework(root: &Path) {
    for rel in [
        "adapters/sources",
        "adapters/targets",
        "adapters/shared",
        "plugins",
        "plugins/test/skills",
    ] {
        fs::create_dir_all(root.join(rel)).expect("scaffold dir");
    }

    let marketplace = root.join(".cursor-plugin").join("marketplace.json");
    fs::create_dir_all(marketplace.parent().expect("marketplace parent"))
        .expect("mkdir .cursor-plugin");
    fs::write(
        &marketplace,
        r#"{
  "name": "test",
  "owner": { "name": "Test Owner", "email": "test@example.com" },
  "metadata": {
    "description": "Synthetic marketplace for specify lint framework e2e tests.",
    "version": "0.0.0",
    "pluginRoot": "plugins"
  },
  "plugins": [
    {
      "name": "test",
      "source": "test",
      "description": "Synthetic plugin used by specify lint framework e2e tests."
    }
  ]
}
"#,
    )
    .expect("marketplace.json");

    let plugin_manifest =
        root.join("plugins").join("test").join(".cursor-plugin").join("plugin.json");
    fs::create_dir_all(plugin_manifest.parent().expect("plugin manifest parent"))
        .expect("mkdir plugins/test/.cursor-plugin");
    fs::write(
        &plugin_manifest,
        r#"{
  "name": "test",
  "displayName": "Test Plugin",
  "description": "Synthetic plugin used by specify lint framework e2e tests.",
  "version": "0.0.0"
}
"#,
    )
    .expect("plugins/test/.cursor-plugin/plugin.json");

    let standards = root.join("docs").join("standards").join("skill-authoring.md");
    fs::create_dir_all(standards.parent().expect("standards parent"))
        .expect("mkdir docs/standards");
    fs::write(
        &standards,
        "# Skill authoring (synthetic)\n\nDescription cap: 512 characters. Body cap: 200 lines.\n",
    )
    .expect("skill-authoring.md");

    let canonical = root.join("docs").join("reference").join("review-team-protocol.md");
    fs::create_dir_all(canonical.parent().expect("canonical parent"))
        .expect("mkdir docs/reference");
    fs::write(&canonical, "# Review Team Protocol\n\nSynthetic stub for tests.\n")
        .expect("review-team-protocol.md");
}

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

/// `specify lint framework --output-format json` emits a wire envelope that
/// passes the binary's own pre-emit schema validation (the
/// diagnostics JSON formatter validates against
/// `DIAGNOSTIC_REPORT_JSON_SCHEMA` before it returns; a validation
/// failure would have exited 1 with `review-envelope-schema` on
/// stderr instead of producing parseable stdout). The test reads
/// stdout, asserts it parses, and pins the closed top-level shape.
#[test]
fn json_envelope_validates_against_schema() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());

    let (_code, stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "json"]);

    let envelope: Value = serde_json::from_slice(&stdout).unwrap_or_else(|err| {
        panic!("stdout is not JSON: {err}; stderr:\n{}", String::from_utf8_lossy(&stderr))
    });

    let stderr_text = String::from_utf8_lossy(&stderr);
    assert!(
        !stderr_text.contains("review-envelope-schema"),
        "binary surfaced a schema-validation failure on stderr: {stderr_text}",
    );

    assert_eq!(
        envelope.get("version").and_then(Value::as_u64),
        Some(1),
        "envelope must carry the v1 discriminant",
    );
    assert!(envelope.get("summary").is_some(), "envelope must carry a summary tally");
    assert!(
        envelope.get("findings").and_then(Value::as_array).is_some(),
        "envelope must carry a findings array",
    );
    let object = envelope.as_object().expect("envelope is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["findings", "summary", "version"],
        "envelope must carry exactly the v1 top-level keys",
    );
}

/// One `lint-completed` event lands in
/// `<framework_root>/.specify/journal.jsonl` per run.
/// The payload shape (`scope.target: None`, `scope.slice: None`,
/// `baseline_present: false`) is pinned alongside the existence
/// check.
#[test]
fn lint_completed_event_lands_in_journal() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());

    let journal_path = temp.path().join(".specify").join("journal.jsonl");
    assert!(!journal_path.exists(), "precondition: journal must not exist before the run");

    let (_code, _stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "json"]);

    assert!(
        journal_path.is_file(),
        "expected journal at {}; stderr:\n{}",
        journal_path.display(),
        String::from_utf8_lossy(&stderr),
    );

    let raw = fs::read_to_string(&journal_path).expect("read journal");
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one event per run; got {} lines:\n{raw}",
        lines.len(),
    );

    let event: Value = serde_json::from_str(lines[0]).expect("event parses as JSON");
    assert_eq!(
        event.get("event").and_then(Value::as_str),
        Some("lint-completed"),
        "first event must be lint-completed; got {event}",
    );
    let payload = event.get("payload").expect("event has payload");
    let scope = payload.get("scope").expect("payload has scope");
    assert!(scope.get("target").is_some_and(Value::is_null), "scope.target must be null");
    assert!(scope.get("slice").is_some_and(Value::is_null), "scope.slice must be null");
    assert!(scope.get("artifact").is_some_and(Value::is_null), "scope.artifact must be null");
    assert_eq!(
        payload.get("baseline_present").and_then(Value::as_bool),
        Some(false),
        "baseline_present must be false",
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
/// rule (now including the `rules` WASI tool that owns CORE-026) is
/// pre-empted, so no `rules.*` finding can surface through the binary on a
/// duplicate-id tree. The run still completes (exit 0) with the skip
/// signalled on stderr. Pinned so Phase 7 (CORE-026 -> rules tool)
/// accounts for the shadowing rather than assuming the tool ever fired
/// here.
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

/// `--output-format pretty` produces a non-empty stdout body that
/// includes the diagnostics-formatter header — confirms the four
/// formatters from [`specify_diagnostics`] are wired
/// into the `specify lint framework` verb.
#[test]
fn pretty_format_emits_diagnostics_summary() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());

    let (_code, stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "pretty"]);
    let stdout = String::from_utf8(stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("finding(s)") && stdout.contains("Summary:"),
        "expected pretty diagnostics body; got:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&stderr),
    );
}
