//! Integration tests for `specify capability {resolve, check, pipeline}`.
//!
//! `specify capability resolve` and the `Cached` source flavour of
//! `capability resolve` get extra coverage in `tests/e2e.rs`; this file
//! focuses on `capability pipeline` (the verb the define / build / merge
//! skill rewrites drive directly) and the `capability check` happy path.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::{TempDir, tempdir};

mod common;
use common::{copy_dir, parse_json, repo_root, specify};

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
fn pipeline_define_lists_briefs_in_order() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "pipeline", "define"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["envelope-version"], 6);
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
fn pipeline_build_and_merge_each_have_brief() {
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
fn pipeline_phase_plan_lists_briefs_in_topo() {
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
fn pipeline_phase_plan_empty_without_block() {
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
fn pipeline_phase_plan_preserves_define() {
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
fn pipeline_with_slice_reports_completion() {
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
fn check_succeeds_on_omnia_yaml() {
    let assert = specify()
        .args(["--format", "json", "capability", "check"])
        .arg(repo_root().join("schemas").join("omnia"))
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["envelope-version"], 6);
    assert_eq!(value["passed"], true, "omnia fixture must validate clean: {value}");
}

#[test]
fn check_text_says_ok() {
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
