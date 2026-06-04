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
use tempfile::TempDir;

/// Scaffold a minimal framework tree that passes
/// `specify_standards::framework::context::Context::from_framework_root` and
/// supplies the marketplace + canonical-doc files the imperative
/// `Check` predicates expect. Intentionally identical in shape to
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

/// Write the migrated `CORE-042` rule that drives the retired
/// imperative `skill.missing-frontmatter` predicate through the RFC-31
/// `kind: authoring-predicate` bridge. Mirrors the live rule shape in
/// `augentic/specify`'s `adapters/shared/rules/core/`.
fn write_authoring_predicate_rule(root: &Path) {
    let path = root.join("adapters/shared/rules/core/CORE-042-skill-missing-frontmatter.md");
    fs::create_dir_all(path.parent().expect("core rules dir")).expect("mkdir core rules");
    fs::write(
        &path,
        "---\n\
id: CORE-042\n\
title: Skill Missing Frontmatter\n\
severity: important\n\
trigger: SKILL.md is missing YAML frontmatter.\n\
rule_hints:\n\
\x20 - kind: authoring-predicate\n\
\x20   value: skill.missing-frontmatter\n\
---\n\n\
## Rule\n\n\
Delegates to the imperative `skill.missing-frontmatter` predicate via the RFC-31 bridge.\n",
    )
    .expect("write CORE-042 rule");
}

/// Author a SKILL.md under `plugins/test/skills/<name>/`.
fn write_skill(root: &Path, name: &str, body: &str) {
    let path = root.join("plugins").join("test").join("skills").join(name).join("SKILL.md");
    fs::create_dir_all(path.parent().expect("skill parent")).expect("mkdir skill parent");
    fs::write(&path, body).expect("write SKILL.md");
}

/// Parse the framework run's stdout envelope, panicking with stderr
/// context on a non-JSON body.
fn envelope(stdout: &[u8], stderr: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|err| {
        panic!("stdout is not JSON: {err}; stderr:\n{}", String::from_utf8_lossy(stderr))
    })
}

/// A migrated `CORE-*` rule carrying `kind: authoring-predicate` resolves
/// and fires through the bridge: a frontmatter-less SKILL.md surfaces a
/// `CORE-042` finding and blocks the run (exit 2).
#[test]
fn authoring_predicate_rule_fires() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_authoring_predicate_rule(temp.path());
    write_skill(temp.path(), "broken", "# Broken Skill\n\nThis SKILL.md has no frontmatter.\n");

    let (code, stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "json"]);
    let envelope = envelope(&stdout, &stderr);
    let findings = envelope.get("findings").and_then(Value::as_array).expect("findings array");
    let core_042: Vec<&Value> = findings
        .iter()
        .filter(|f| f.get("rule-id").and_then(Value::as_str) == Some("CORE-042"))
        .collect();
    assert_eq!(
        core_042.len(),
        1,
        "the authoring-predicate rule must surface exactly one CORE-042 finding; got envelope:\n{envelope:#}",
    );
    assert_eq!(
        core_042[0].get("impact").and_then(Value::as_str),
        Some("Authoring check 'skill.missing-frontmatter' failed."),
        "the finding's impact must carry the bridged predicate id",
    );
    assert_eq!(code, Some(2), "a firing important finding blocks with exit 2");
}

/// The same rule passes when the predicate finds nothing to flag: a
/// tree with no authored skills yields no `CORE-042` finding, proving
/// the bridge evaluates (rather than unconditionally emitting).
#[test]
fn authoring_predicate_clean_tree() {
    let temp = TempDir::new().expect("tempdir");
    scaffold_framework(temp.path());
    write_authoring_predicate_rule(temp.path());

    let (_code, stdout, stderr) = run_lint_framework(temp.path(), &["--output-format", "json"]);
    let envelope = envelope(&stdout, &stderr);
    let findings = envelope.get("findings").and_then(Value::as_array).expect("findings array");
    assert!(
        !findings.iter().any(|f| f.get("rule-id").and_then(Value::as_str) == Some("CORE-042")),
        "no skill authored means the predicate passes; got envelope:\n{envelope:#}",
    );
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
