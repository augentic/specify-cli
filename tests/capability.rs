//! Integration tests for `specify capability {resolve, check, pipeline}`.
//!
//! `specify capability resolve` and the `Cached` source flavour of
//! `capability resolve` get extra coverage in `tests/e2e.rs`; this file
//! focuses on `capability pipeline` (the verb the define / build / merge
//! skill rewrites drive directly), the `capability check` happy path,
//! and the RFC-13 §Migration `schema-became-capability` diagnostic that
//! fires when a directory carries only the pre-RFC-13 manifest.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn parse_json(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"))
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create_dir_all dst");
    for entry in fs::read_dir(src).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let kind = entry.file_type().expect("file_type");
        let target = dst.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
    fn init() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        specify()
            .current_dir(&root)
            .args(["init"])
            .arg(repo_root().join("schemas").join("omnia"))
            .args(["--name", "test-proj"])
            .assert()
            .success();
        copy_dir(&repo_root().join("schemas/omnia"), &root.join("schemas/omnia"));
        Self { _tmp: tmp, root }
    }

    /// Initialise a project backed by a local fixture capability dir.
    /// The fixture is mirrored into `<tmp>/schemas/<name>/` so that
    /// subsequent `specify` invocations resolve it via the usual
    /// `schemas/<name>/` probe.
    fn init_from_fixture(name: &str, fixture_dir: &Path) -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        copy_dir(fixture_dir, &root.join("schemas").join(name));
        specify()
            .current_dir(&root)
            .args(["init"])
            .arg(root.join("schemas").join(name))
            .args(["--name", "test-proj"])
            .assert()
            .success();
        Self { _tmp: tmp, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

#[test]
fn capability_pipeline_define_lists_omnia_define_briefs_in_order() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "define"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["phase"], "define");
    assert_eq!(value["slice"], Value::Null);

    let briefs = value["briefs"].as_array().expect("briefs array");
    let ids: Vec<&str> = briefs.iter().map(|b| b["id"].as_str().unwrap()).collect();
    // Omnia's pipeline.define is [proposal, specs, design, tasks] and
    // `needs` is satisfied by that ordering, so topo order preserves it.
    assert_eq!(ids, vec!["proposal", "specs", "design", "tasks"]);

    for b in briefs {
        assert!(b["id"].is_string());
        assert!(b["description"].is_string());
        assert!(b["path"].is_string());
        assert!(b["needs"].is_array());
        assert!(b["present"] == Value::Null);
    }
}

#[test]
fn capability_pipeline_build_and_merge_each_have_their_brief() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "build"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let ids: Vec<&str> =
        value["briefs"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["build"]);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "merge"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let ids: Vec<&str> =
        value["briefs"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["merge"]);
}

#[test]
fn capability_pipeline_phase_plan_lists_plan_briefs_in_topo_order() {
    let fixture = repo_root().join("tests/fixtures/schema/plan-pipeline");
    let project = Project::init_from_fixture("plan-pipeline", &fixture);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "plan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["phase"], "plan");
    assert_eq!(value["slice"], Value::Null);

    let briefs = value["briefs"].as_array().expect("briefs array");
    let ids: Vec<&str> = briefs.iter().map(|b| b["id"].as_str().unwrap()).collect();
    // discovery feeds propose (needs: [discovery]), so topo order is
    // discovery → propose and must match the declared pipeline.plan
    // order in the fixture manifest.
    assert_eq!(ids, vec!["discovery", "propose"]);

    assert_eq!(briefs[0]["generates"], "discovery.md");
    assert_eq!(briefs[1]["generates"], "propose.md");
    assert_eq!(briefs[1]["needs"].as_array().unwrap()[0], "discovery");
}

#[test]
fn capability_pipeline_phase_plan_is_empty_for_capabilities_without_plan_block() {
    // Omnia (the in-repo capability) does not declare pipeline.plan at
    // all. Asking for --phase plan must succeed and return an empty
    // briefs list rather than erroring out, so callers can probe for
    // plan support without conditional logic.
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "plan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["phase"], "plan");
    assert_eq!(value["briefs"].as_array().unwrap().len(), 0);
}

#[test]
fn capability_pipeline_phase_plan_does_not_perturb_define_output() {
    // Regression: adding pipeline.plan (with briefs before the define
    // phase in load order) must not change what `--phase define`
    // returns.
    let fixture = repo_root().join("tests/fixtures/schema/plan-pipeline");
    let project = Project::init_from_fixture("plan-pipeline", &fixture);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "define"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let ids: Vec<&str> =
        value["briefs"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["proposal"]);
}

#[test]
fn capability_pipeline_with_slice_reports_completion() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.root().join(".specify/slices/my-slice");
    fs::write(slice_dir.join("proposal.md"), "# Proposal\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "capability",
            "pipeline",
            "define",
            "--slice",
            slice_dir.to_str().unwrap(),
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let briefs = value["briefs"].as_array().unwrap();
    let presence: std::collections::BTreeMap<&str, bool> = briefs
        .iter()
        .filter_map(|b| {
            let id = b["id"].as_str()?;
            let present = b["present"].as_bool()?;
            Some((id, present))
        })
        .collect();
    assert_eq!(presence.get("proposal"), Some(&true));
    assert_eq!(presence.get("design"), Some(&false));
    assert_eq!(presence.get("tasks"), Some(&false));
}

// ---- specify capability check ------------------------------------------------

#[test]
fn capability_check_succeeds_on_omnia_capability_yaml() {
    let assert = specify()
        .args(["--format", "json", "capability", "check"])
        .arg(repo_root().join("schemas").join("omnia"))
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["passed"], true, "omnia fixture must validate clean: {value}");
}

#[test]
fn capability_check_text_output_says_capability_ok() {
    let assert = specify()
        .args(["capability", "check"])
        .arg(repo_root().join("schemas").join("omnia"))
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        stdout.contains("Capability OK"),
        "text mode must use the post-RFC-13 noun, got: {stdout}"
    );
}

// ---- RFC-13 §Migration: schema-became-capability ---------------------------

/// Regression: a directory that carries ONLY the pre-RFC-13
/// `schema.yaml` (no `capability.yaml`) must be refused with a clear
/// `schema-became-capability` diagnostic. The message must name the
/// offending file path, cite RFC-13, and point the operator at the
/// post-RFC-13 init command.
#[test]
fn capability_check_refuses_legacy_schema_yaml_with_schema_became_capability() {
    let tmp = tempdir().expect("tempdir");
    let manifest_dir = tmp.path();
    let legacy = manifest_dir.join("schema.yaml");
    fs::write(
        &legacy,
        "name: legacy\nversion: 1\ndescription: pre-RFC-13 manifest\n\
         pipeline:\n  define: []\n  build: []\n  merge: []\n",
    )
    .expect("write legacy schema.yaml");
    assert!(legacy.is_file(), "test precondition: schema.yaml must exist");
    assert!(
        !manifest_dir.join("capability.yaml").exists(),
        "test precondition: capability.yaml must be absent"
    );

    let assert = specify()
        .args(["--format", "json", "capability", "check"])
        .arg(manifest_dir)
        .assert()
        .failure();

    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 1, "schema-became-capability must exit non-zero (1)");

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(
        value["error"], "schema-became-capability",
        "JSON envelope must carry the stable kebab-case error code, got: {value}"
    );
    let msg = value["message"].as_str().expect("message string");
    assert!(
        msg.contains("schema-became-capability"),
        "message must surface the stable code, got: {msg}"
    );
    assert!(msg.contains("RFC-13"), "message must cite RFC-13, got: {msg}");
    assert!(
        msg.contains("capability.yaml"),
        "message must name the post-rename filename, got: {msg}"
    );
    assert!(msg.contains("schema.yaml"), "message must surface the offending file, got: {msg}");
    assert!(
        msg.contains("specify init <capability>"),
        "message must hint at the post-rename init command, got: {msg}"
    );
    assert!(
        msg.contains("rfcs/rfc-13-extensibility.md#migration"),
        "message must link RFC-13 §Migration, got: {msg}"
    );
}

#[test]
fn capability_check_resolves_capability_yaml_when_both_files_present() {
    // A directory carrying both manifests (e.g. the in-repo
    // `schemas/omnia/` during the cut-over) must load the post-RFC-13
    // `capability.yaml` and never trip the `schema-became-capability`
    // diagnostic. This pins the priority order documented on
    // `Capability::manifest_path_in`. We seed `schema.yaml` with a
    // structurally-invalid manifest (empty pipeline phases violate
    // `capability.schema.json`) so a regression that loaded the
    // wrong file would surface as a `passed: false` validation
    // failure rather than as silent success.
    let tmp = tempdir().expect("tempdir");
    let dir = tmp.path();
    fs::write(
        dir.join("schema.yaml"),
        "name: from-schema\nversion: 1\ndescription: pre-RFC-13 (invalid)\n\
         pipeline:\n  define: []\n  build: []\n  merge: []\n",
    )
    .expect("write schema.yaml");
    fs::write(
        dir.join("capability.yaml"),
        "name: from-capability\nversion: 1\ndescription: post-RFC-13\n\
         pipeline:\n  define:\n    - id: proposal\n      brief: briefs/proposal.md\n\
         \x20\x20build:\n    - id: build\n      brief: briefs/build.md\n\
         \x20\x20merge:\n    - id: merge\n      brief: briefs/merge.md\n",
    )
    .expect("write capability.yaml");

    let assert =
        specify().args(["--format", "json", "capability", "check"]).arg(dir).assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["passed"], true, "capability.yaml content must win, got: {value}");
}

// ---- specify schema * is gone ----------------------------------------------

#[test]
fn schema_subcommand_is_gone_from_top_level_help() {
    let assert = specify().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        stdout.contains("capability"),
        "post-RFC-13 --help must list `capability`, got:\n{stdout}"
    );
    // `--help` lists subcommand names one-per-line (clap default).
    // A grepped `\n  schema` would catch the surface even if `schema`
    // appeared incidentally inside descriptions of other commands.
    assert!(
        !stdout
            .lines()
            .any(|line| line.trim_start().starts_with("schema ") || line.trim_start() == "schema"),
        "pre-RFC-13 `schema` subcommand must be gone from --help, got:\n{stdout}"
    );
}

#[test]
fn schema_subcommand_returns_clap_unrecognised_subcommand_error() {
    let assert = specify().args(["schema", "check", "schemas/omnia"]).assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8 stderr");
    // clap's standard "unrecognised subcommand" message includes the
    // word "unrecognized" or "unexpected" depending on version; either
    // is acceptable. Anchor on the noun that proves clap rejected it
    // rather than dispatching to a real handler.
    assert!(
        stderr.to_lowercase().contains("unrecognized")
            || stderr.to_lowercase().contains("unrecognised")
            || stderr.to_lowercase().contains("unexpected argument")
            || stderr.contains("error: ") && stderr.contains("schema"),
        "pre-RFC-13 `specify schema *` must be a clap-level error, got stderr:\n{stderr}"
    );
}
