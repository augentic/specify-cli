//! End-to-end binary tests for `specrun lint run` (`specrun
//! review` (Phase 2 CLI)").
//!
//! Exercises the wired clap surface, `--rules-root` resolution per rules-root resolution,
//! the `--dump-model` debug branch, and the lint exit mapping exit-code map for the
//! `rules-root-required` negative scenario. The deterministic happy
//! path uses a single shared `kind: regex` UNI-100 rule that matches a
//! literal `TODO` token in the project — chosen because the regex
//! evaluator is the simplest Phase 2 hint that surfaces an
//! `important` finding without requiring a WASI tool to be built.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use jsonschema::{Registry, Resource, Validator};
use serde_json::Value;
use specify_schema::{
    DIAGNOSTIC_JSON_SCHEMA, DIAGNOSTIC_REPORT_JSON_SCHEMA, WORKSPACE_MODEL_JSON_SCHEMA,
};
use tempfile::TempDir;

const FINDING_SCHEMA_URL: &str =
    "https://github.com/augentic/specify-cli/schemas/diagnostics/diagnostic.schema.json";

/// Compile the diagnostic-report envelope schema with the
/// `diagnostic.schema.json` child resource wired through a
/// `jsonschema::Registry`. Mirrors the
/// `specify_lints::lint::diagnostics::json::render_value` setup so the
/// e2e test re-validates the same shape the CLI emits.
fn compile_review_result_validator() -> Validator {
    let envelope: Value =
        serde_json::from_str(DIAGNOSTIC_REPORT_JSON_SCHEMA).expect("envelope schema");
    let finding: Value = serde_json::from_str(DIAGNOSTIC_JSON_SCHEMA).expect("finding schema");
    let registry = Registry::new()
        .add(FINDING_SCHEMA_URL, Resource::from_contents(finding))
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry build");
    jsonschema::options().with_registry(&registry).build(&envelope).expect("validator build")
}

fn compile_workspace_model_validator() -> Validator {
    let schema: Value = serde_json::from_str(WORKSPACE_MODEL_JSON_SCHEMA).expect("parse schema");
    jsonschema::validator_for(&schema).expect("validator build")
}

#[track_caller]
fn assert_validates(validator: &Validator, stdout: &str, schema_label: &str) {
    let instance: Value = serde_json::from_str(stdout)
        .unwrap_or_else(|err| panic!("stdout is not JSON ({err}); raw:\n{stdout}"));
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(
        errors.is_empty(),
        "stdout failed schema validation ({schema_label}): {errors:?}; raw:\n{stdout}"
    );
}

/// Scratch workspace used by the happy-path scenarios.
///
/// Lives in the tempdir until the test returns. Two trees are produced:
///
/// - `project_dir/` — a minimal initialised project (`.specify/project.yaml`
///   declaring the `contract` tool so the `kind: tool` hint family at
///   least passes the `kind: tool` evaluator contract `is_declared` half) plus a `notes.md` file
///   carrying the literal `TODO` token the UNI-100 regex hint matches.
/// - `codex_dir/` — a fresh rules tree with one shared rule under
///   `adapters/shared/rules/universal/uni-100.md`. The rule's
///   `kind: regex` hint pattern is `TODO`.
struct Fixture {
    _root: TempDir,
    project: std::path::PathBuf,
    codex: std::path::PathBuf,
}

fn build_fixture() -> Fixture {
    let root = TempDir::new().expect("create tempdir");
    let project = root.path().join("project");
    let codex = root.path().join("rules");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::create_dir_all(codex.join("adapters/shared/rules/universal")).expect("mkdir codex");

    fs::write(
        project.join(".specify").join("project.yaml"),
        concat!(
            "name: review-e2e\n",
            "tools:\n",
            "  - name: contract\n",
            "    version: 0.1.0\n",
            "    source: https://example.com/contract.wasm\n",
        ),
    )
    .expect("write project.yaml");

    fs::write(project.join("notes.md"), "# Project notes\n\nTODO: drop scaffolding.\n")
        .expect("write notes.md");

    fs::write(
        codex.join("adapters/shared/rules/universal/uni-100.md"),
        concat!(
            "---\n",
            "id: UNI-100\n",
            "title: Forbid scaffolding TODOs\n",
            "severity: important\n",
            "trigger: TODO comments leak development scaffolding into shipped artefacts.\n",
            "lint_mode: deterministic\n",
            "deterministic_hints:\n",
            "  - kind: regex\n",
            "    value: TODO\n",
            "---\n",
            "## Rule\n",
            "\n",
            "Strip scaffolding TODOs before merge.\n",
        ),
    )
    .expect("write UNI-100");

    Fixture {
        _root: root,
        project,
        codex,
    }
}

fn run_review(project: &Path, codex: Option<&Path>, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::cargo_bin("specrun").expect("cargo_bin(specrun)");
    // The global `--format` toggles the error-envelope shape; the
    // per-subcommand `--output-format` selects the the diagnostics formatter set closed set.
    cmd.arg("--format").arg("json");
    cmd.arg("lint").arg("run");
    cmd.arg("--target").arg("omnia");
    cmd.arg("--project-dir").arg(project);
    cmd.arg("--output-format").arg("json");
    if let Some(codex) = codex {
        cmd.arg("--rules-root").arg(codex);
    }
    cmd.env_remove("RULES_ROOT");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.output().expect("specrun invocation")
}

/// Happy path: a single `important` finding from the UNI-100 regex
/// hint lands stdout on a schema-valid review envelope and exits 2 per
/// lint exit mapping.
#[test]
fn review_emits_important_finding_and_exits_2() {
    let fx = build_fixture();
    let output = run_review(&fx.project, Some(&fx.codex), &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    let validator = compile_review_result_validator();
    assert_validates(&validator, stdout, "review-result");
    let envelope: Value = serde_json::from_str(stdout).expect("parse envelope");
    let important = envelope
        .pointer("/summary/important")
        .and_then(Value::as_u64)
        .expect("summary.important present");
    assert!(
        important >= 1,
        "expected ≥1 important finding, got {important}; envelope:\n{envelope:#}"
    );

    let rule_id = envelope
        .pointer("/findings/0/rule-id")
        .and_then(Value::as_str)
        .expect("findings[0].rule-id");
    assert_eq!(rule_id, "UNI-100", "envelope:\n{envelope:#}");
}

/// `DiagnosticReport` envelope byte-stability: two back-to-back runs against the same fixture
/// must emit byte-identical stdout. Pins the deterministic ordering
/// contract through the CLI boundary.
#[test]
fn review_run_byte_stable() {
    let fx = build_fixture();
    let first = run_review(&fx.project, Some(&fx.codex), &[]);
    let second = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        first.stdout, second.stdout,
        "consecutive specrun lint runs must emit byte-identical stdout"
    );
}

/// `--dump-model` skips evaluation, emits a `WorkspaceModel` envelope
/// that validates against the workspace-model schema, and exits 0.
#[test]
fn review_dump_model_exits_0() {
    let fx = build_fixture();
    let output = run_review(&fx.project, Some(&fx.codex), &["--dump-model"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "expected exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    let validator = compile_workspace_model_validator();
    assert_validates(&validator, stdout, "workspace-model");
}

/// Journal event: every completed scan appends one
/// `lint-completed` line to `.specify/journal.jsonl` with the closed
/// `snake_case` payload shape. The fixture wires a Markdown directive
/// that demotes the UNI-100 TODO finding so the asserted counts
/// straddle both buckets (`ignored: 1`, `open: 0`) and the scan exits
/// clean (`exit_code: 0`) — proving the journal `exit_code` mirrors
/// the status-aware exit decision the exit and presentation
/// semantics define.
#[test]
fn emits_lint_completed_event() {
    use std::path::PathBuf;

    let root = TempDir::new().expect("create tempdir");
    let project: PathBuf = root.path().join("project");
    let codex: PathBuf = root.path().join("rules");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::create_dir_all(codex.join("adapters/shared/rules/universal")).expect("mkdir codex");

    fs::write(project.join(".specify").join("project.yaml"), "name: review-journal-e2e\n")
        .expect("write project.yaml");

    // `<!-- specify-ignore: UNI-100 — … -->` lands on line 2 so the
    // directive's `target_line` resolves to the next non-blank,
    // non-comment line: the TODO on line 3.
    fs::write(
        project.join("notes.md"),
        concat!(
            "# Project notes\n",
            "<!-- specify-ignore: UNI-100 — accepted tech-debt sentinel for the demo -->\n",
            "TODO: drop scaffolding.\n",
        ),
    )
    .expect("write notes.md");

    fs::write(
        codex.join("adapters/shared/rules/universal/uni-100.md"),
        concat!(
            "---\n",
            "id: UNI-100\n",
            "title: Forbid scaffolding TODOs\n",
            "severity: important\n",
            "trigger: TODO comments leak development scaffolding into shipped artefacts.\n",
            "lint_mode: deterministic\n",
            "deterministic_hints:\n",
            "  - kind: regex\n",
            "    value: TODO\n",
            "---\n",
            "## Rule\n\nStrip scaffolding TODOs before merge.\n",
        ),
    )
    .expect("write UNI-100");

    let output = run_review(&project, Some(&codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "directive demotes the only finding to `ignored`; scan must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let raw = fs::read_to_string(project.join(".specify").join("journal.jsonl"))
        .expect("read journal.jsonl");
    let last_line =
        raw.lines().rfind(|l| !l.is_empty()).expect("journal must contain at least one line");
    let event: Value = serde_json::from_str(last_line)
        .unwrap_or_else(|err| panic!("last journal line is not JSON ({err}): {last_line}"));

    assert_eq!(
        event.pointer("/event").and_then(Value::as_str),
        Some("lint-completed"),
        "last journal line must be the lint-completed event; got:\n{event:#}",
    );

    let payload = event.get("payload").expect("payload object present");
    assert_eq!(payload.pointer("/scope/target").and_then(Value::as_str), Some("omnia"));
    assert!(
        payload.pointer("/scope/slice").is_some_and(Value::is_null),
        "slice must serialise to JSON null when --slice is absent; payload:\n{payload:#}",
    );
    assert!(
        payload.pointer("/scope/artifact").is_some_and(Value::is_null),
        "artifact must serialise to JSON null on full scans; payload:\n{payload:#}",
    );
    assert_eq!(
        payload.pointer("/counts/open").and_then(Value::as_u64),
        Some(0),
        "the only finding is directive-demoted; open bucket must be empty: {payload:#}",
    );
    assert_eq!(
        payload.pointer("/counts/ignored").and_then(Value::as_u64),
        Some(1),
        "the directive demotes UNI-100; ignored bucket must be 1: {payload:#}",
    );
    assert_eq!(payload.pointer("/counts/false_positive").and_then(Value::as_u64), Some(0));
    assert_eq!(payload.pointer("/baseline_present").and_then(Value::as_bool), Some(false));
    assert_eq!(payload.pointer("/exit_code").and_then(Value::as_i64), Some(0));
    assert!(
        payload.pointer("/duration_ms").and_then(Value::as_u64).is_some(),
        "duration_ms must be present and serialise as a JSON number: {payload:#}",
    );

    for forbidden in ["duration-ms", "baseline-present", "false-positive", "exit-code"] {
        assert!(
            !last_line.contains(&format!("\"{forbidden}\"")),
            "lint-completed payload must use snake_case field names; raw:\n{last_line}",
        );
    }
}

/// rules-root resolution / lint exit mapping negative: with no `--rules-root`, no project-local
/// `adapters/shared/rules/universal/` rung, and no
/// `.specify/cache/rules/` cache, the resolver returns
/// `rules-root-required`. The CLI surfaces it on stderr and exits 2.
#[test]
fn review_missing_rules_root_exits_2() {
    let project_root = TempDir::new().expect("project tempdir");
    let project = project_root.path().join("project");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::write(project.join(".specify").join("project.yaml"), "name: review-e2e-missing\n")
        .expect("write project.yaml");

    // Pass `--format json` so the failure envelope renders as JSON on
    // stderr (with the kebab-case `rule-id` field). The text branch
    // collapses to `error: validation failed: N errors` and would
    // hide the closed `rules-root-required` discriminant.
    let mut cmd = Command::cargo_bin("specrun").expect("cargo_bin(specrun)");
    cmd.arg("--format")
        .arg("json")
        .arg("lint")
        .arg("run")
        .arg("--target")
        .arg("omnia")
        .arg("--project-dir")
        .arg(&project)
        .arg("--output-format")
        .arg("json")
        .env_remove("RULES_ROOT");
    let output = cmd.output().expect("specrun invocation");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = std::str::from_utf8(&output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("rules-root-required"),
        "stderr must mention rules-root-required; got:\n{stderr}"
    );
}
