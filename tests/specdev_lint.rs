//! End-to-end test for the `specdev lint` extension.
//!
//! Exercises the binary surface added for framework convergence:
//!
//! - `specdev lint --output-format json` against a small
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
//! The scaffold mirrors `tests/specdev_check_json.rs::write_scaffold`
//! and is deliberately small: just enough framework structure to
//! satisfy `Context::is_framework_root` and silence the marketplace
//! / agent-teams predicates so the JSON envelope shape — not
//! individual finding contents — is what this test pins.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

/// Scaffold a minimal framework tree that passes
/// `specify_lints::framework::context::Context::from_framework_root` and
/// supplies the marketplace + canonical-doc files the imperative
/// `Check` predicates expect. Intentionally identical in shape to
/// the scaffold used by `tests/specdev_check_json.rs` so both
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
    "description": "Synthetic marketplace for specdev lint e2e tests.",
    "version": "0.0.0",
    "pluginRoot": "plugins"
  },
  "plugins": [
    {
      "name": "test",
      "source": "test",
      "description": "Synthetic plugin used by specdev lint e2e tests."
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
  "description": "Synthetic plugin used by specdev lint e2e tests.",
  "version": "0.0.0"
}
"#,
    )
    .expect("plugins/test/.cursor-plugin/plugin.json");

    let skill_schema = root.join(".cursor").join("schemas").join("skill.schema.json");
    fs::create_dir_all(skill_schema.parent().expect("skill schema parent"))
        .expect("mkdir .cursor/schemas");
    fs::write(
        &skill_schema,
        r#"{
  "type": "object",
  "properties": {
    "description": { "type": "string", "maxLength": 512 }
  }
}
"#,
    )
    .expect("skill.schema.json");

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

/// Run `specdev lint --framework-root <root> --output-format json`
/// and return the captured `(exit, stdout, stderr)` triple.
fn run_specdev_lint(root: &Path, args: &[&str]) -> (Option<i32>, Vec<u8>, Vec<u8>) {
    let output = Command::cargo_bin("specdev")
        .expect("cargo_bin(specdev)")
        .args(["lint", "--framework-root"])
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("specdev invocation");
    (output.status.code(), output.stdout, output.stderr)
}

/// `specdev lint --output-format json` emits a wire envelope that
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

    let (_code, stdout, stderr) = run_specdev_lint(temp.path(), &["--output-format", "json"]);

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

    let (_code, _stdout, stderr) = run_specdev_lint(temp.path(), &["--output-format", "json"]);

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

/// `--output-format pretty` produces a non-empty stdout body that
/// includes the diagnostics-formatter header — confirms the four
/// formatters from [`specify_diagnostics`] are wired
/// into the `specdev lint` verb.
#[test]
fn pretty_format_emits_diagnostics_summary() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());

    let (_code, stdout, stderr) = run_specdev_lint(temp.path(), &["--output-format", "pretty"]);
    let stdout = String::from_utf8(stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("finding(s)") && stdout.contains("Summary:"),
        "expected pretty diagnostics body; got:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&stderr),
    );
}
