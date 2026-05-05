//! Integration tests for the `specify schema pipeline` subcommand.
//!
//! `schema resolve` and `schema check` already have coverage in
//! `tests/e2e.rs`; this file focuses on the new `pipeline` verb, which
//! is what the define / build / merge skill rewrites drive directly.

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
            .args(["init", "--schema-uri"])
            .arg(repo_root().join("schemas").join("omnia"))
            .args(["--name", "test-proj"])
            .assert()
            .success();
        copy_dir(&repo_root().join("schemas/omnia"), &root.join("schemas/omnia"));
        Self { _tmp: tmp, root }
    }

    /// Initialise a project backed by a local fixture schema dir. The
    /// fixture is mirrored into `<tmp>/schemas/<schema_name>/` so that
    /// subsequent `specify` invocations resolve it via the usual
    /// `schemas/<name>/` probe.
    fn init_from_fixture(schema_name: &str, fixture_dir: &Path) -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        copy_dir(fixture_dir, &root.join("schemas").join(schema_name));
        specify()
            .current_dir(&root)
            .args(["init", "--schema-uri"])
            .arg(root.join("schemas").join(schema_name))
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
fn schema_pipeline_define_lists_omnia_define_briefs_in_order() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "pipeline", "define"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["phase"], "define");
    assert_eq!(value["change"], Value::Null);

    let briefs = value["briefs"].as_array().expect("briefs array");
    let ids: Vec<&str> = briefs.iter().map(|b| b["id"].as_str().unwrap()).collect();
    // Omnia's pipeline.define is [proposal, specs, design, tasks] and
    // `needs` is satisfied by that ordering, so topo order preserves it.
    assert_eq!(ids, vec!["proposal", "specs", "design", "tasks"]);

    // Every brief carries structured frontmatter plus its absolute path.
    for b in briefs {
        assert!(b["id"].is_string());
        assert!(b["description"].is_string());
        assert!(b["path"].is_string());
        assert!(b["needs"].is_array());
        // `generates` may be a string or null; `tracks` the same.
        assert!(b["present"] == Value::Null);
    }
}

#[test]
fn schema_pipeline_build_and_merge_each_have_their_brief() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "pipeline", "build"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let ids: Vec<&str> =
        value["briefs"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["build"]);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "pipeline", "merge"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let ids: Vec<&str> =
        value["briefs"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["merge"]);
}

#[test]
fn schema_pipeline_phase_plan_lists_plan_briefs_in_topo_order() {
    let fixture = repo_root().join("tests/fixtures/schema/plan-pipeline");
    let project = Project::init_from_fixture("plan-pipeline", &fixture);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "pipeline", "plan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["phase"], "plan");
    assert_eq!(value["change"], Value::Null);

    let briefs = value["briefs"].as_array().expect("briefs array");
    let ids: Vec<&str> = briefs.iter().map(|b| b["id"].as_str().unwrap()).collect();
    // discovery feeds propose (needs: [discovery]), so topo order is
    // discovery → propose and must match the declared pipeline.plan
    // order in schema.yaml.
    assert_eq!(ids, vec!["discovery", "propose"]);

    assert_eq!(briefs[0]["generates"], "discovery.md");
    assert_eq!(briefs[1]["generates"], "propose.md");
    assert_eq!(briefs[1]["needs"].as_array().unwrap()[0], "discovery");
}

#[test]
fn schema_pipeline_phase_plan_is_empty_for_schemas_without_plan_block() {
    // Omnia (the in-repo schema) does not declare pipeline.plan at all.
    // Asking for --phase plan must succeed and return an empty briefs
    // list rather than erroring out, so callers can probe for plan
    // support without conditional logic.
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "pipeline", "plan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["phase"], "plan");
    assert_eq!(value["briefs"].as_array().unwrap().len(), 0);
}

#[test]
fn schema_pipeline_phase_plan_does_not_perturb_define_output() {
    // Regression: adding pipeline.plan (with briefs before the define
    // phase in load order) must not change what `--phase define`
    // returns. This is the explicit "Do NOT change the default
    // iteration order of Schema::entries()" constraint in L3.C.
    let fixture = repo_root().join("tests/fixtures/schema/plan-pipeline");
    let project = Project::init_from_fixture("plan-pipeline", &fixture);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "pipeline", "define"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let ids: Vec<&str> =
        value["briefs"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["proposal"]);
}

#[test]
fn schema_pipeline_with_change_reports_completion() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();
    let change_dir = project.root().join(".specify/changes/my-change");
    // Create only the proposal artifact so `present` differs across briefs.
    fs::write(change_dir.join("proposal.md"), "# Proposal\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "schema",
            "pipeline",
            "define",
            "--change",
            change_dir.to_str().unwrap(),
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
