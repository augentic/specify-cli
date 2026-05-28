//! End-to-end binary tests for `specrun lint run` (RFC-32 §"`specrun
//! review` (Phase 2 CLI)").
//!
//! Exercises the wired clap surface, `--codex-root` resolution per §D7,
//! the `--dump-model` debug branch, and the §D8 exit-code map for the
//! `codex-root-required` negative scenario. The deterministic happy
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
    LINT_FINDING_JSON_SCHEMA, LINT_RESULT_JSON_SCHEMA, WORKSPACE_MODEL_JSON_SCHEMA,
};
use tempfile::TempDir;

const FINDING_SCHEMA_URL: &str =
    "https://github.com/augentic/specify-cli/schemas/lint/finding.schema.json";

/// Compile the review-result envelope schema with the `finding.schema.json`
/// child resource wired through a `jsonschema::Registry`. Mirrors the
/// `specify_codex::lint::diagnostics::json::render_value` setup so the
/// e2e test re-validates the same shape the CLI emits.
fn compile_review_result_validator() -> Validator {
    let envelope: Value = serde_json::from_str(LINT_RESULT_JSON_SCHEMA).expect("envelope schema");
    let finding: Value = serde_json::from_str(LINT_FINDING_JSON_SCHEMA).expect("finding schema");
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
///   least passes the §D4 `is_declared` half) plus a `notes.md` file
///   carrying the literal `TODO` token the UNI-100 regex hint matches.
/// - `codex_dir/` — a fresh codex tree with one shared rule under
///   `adapters/shared/codex/universal/uni-100.md`. The rule's
///   `kind: regex` hint pattern is `TODO`.
struct Fixture {
    _root: TempDir,
    project: std::path::PathBuf,
    codex: std::path::PathBuf,
}

fn build_fixture() -> Fixture {
    let root = TempDir::new().expect("create tempdir");
    let project = root.path().join("project");
    let codex = root.path().join("codex");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::create_dir_all(codex.join("adapters/shared/codex/universal")).expect("mkdir codex");

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
        codex.join("adapters/shared/codex/universal/uni-100.md"),
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
    // per-subcommand `--output-format` selects the §D6 closed set.
    cmd.arg("--format").arg("json");
    cmd.arg("lint").arg("run");
    cmd.arg("--target").arg("omnia");
    cmd.arg("--project-dir").arg(project);
    cmd.arg("--output-format").arg("json");
    if let Some(codex) = codex {
        cmd.arg("--codex-root").arg(codex);
    }
    cmd.env_remove("CODEX_ROOT");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.output().expect("specrun invocation")
}

/// Happy path: a single `important` finding from the UNI-100 regex
/// hint lands stdout on a schema-valid review envelope and exits 2 per
/// §D8.
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

/// §D9 byte-stability: two back-to-back runs against the same fixture
/// must emit byte-identical stdout. Pins the deterministic ordering
/// contract through the CLI boundary.
#[test]
fn review_run_is_byte_stable_across_invocations() {
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
fn review_dump_model_emits_workspace_model_and_exits_0() {
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

/// §D7 / §D8 negative: with no `--codex-root`, no project-local
/// `adapters/shared/codex/universal/` rung, and no
/// `.specify/cache/codex/` cache, the resolver returns
/// `codex-root-required`. The CLI surfaces it on stderr and exits 2.
#[test]
fn review_missing_codex_root_exits_2_with_codex_root_required() {
    let project_root = TempDir::new().expect("project tempdir");
    let project = project_root.path().join("project");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::write(project.join(".specify").join("project.yaml"), "name: review-e2e-missing\n")
        .expect("write project.yaml");

    // Pass `--format json` so the failure envelope renders as JSON on
    // stderr (with the kebab-case `rule-id` field). The text branch
    // collapses to `error: validation failed: N errors` and would
    // hide the closed `codex-root-required` discriminant.
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
        .env_remove("CODEX_ROOT");
    let output = cmd.output().expect("specrun invocation");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = std::str::from_utf8(&output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("codex-root-required"),
        "stderr must mention codex-root-required; got:\n{stderr}"
    );
}
