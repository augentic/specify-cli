//! End-to-end binary tests for `specrun review run` (RFC-32 §"`specrun
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
use serde_json::Value;
use specify_schema::{REVIEW_RESULT_JSON_SCHEMA, WORKSPACE_MODEL_JSON_SCHEMA, validate_json_str};
use tempfile::TempDir;

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
            "review_mode: deterministic\n",
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
    cmd.arg("review");
    cmd.arg("run");
    cmd.arg("--target").arg("omnia");
    cmd.arg("--project-dir").arg(project);
    cmd.arg("--format").arg("json");
    if let Some(codex) = codex {
        cmd.arg("--codex-root").arg(codex);
    }
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
    validate_json_str(REVIEW_RESULT_JSON_SCHEMA, stdout).expect("stdout matches review-result schema");

    let envelope: Value = serde_json::from_str(stdout).expect("parse envelope");
    let important = envelope
        .pointer("/summary/important")
        .and_then(Value::as_u64)
        .expect("summary.important present");
    assert!(important >= 1, "expected ≥1 important finding, got {important}; envelope:\n{envelope:#}");

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
        "consecutive specrun review runs must emit byte-identical stdout"
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
    validate_json_str(WORKSPACE_MODEL_JSON_SCHEMA, stdout).expect("stdout matches workspace-model schema");
}

/// §D7 / §D8 negative: a missing codex root surfaces the closed
/// `codex-root-required` rule id and exits 2 via the existing resolver
/// validation path.
#[test]
fn review_missing_codex_root_exits_2_with_codex_root_required() {
    let project_root = TempDir::new().expect("project tempdir");
    let project = project_root.path().join("project");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::write(project.join(".specify").join("project.yaml"), "name: review-e2e-missing\n")
        .expect("write project.yaml");

    let bogus = project_root.path().join("does-not-exist");
    let output = run_review(&project, Some(&bogus), &[]);

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
