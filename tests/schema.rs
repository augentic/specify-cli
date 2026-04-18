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
            .args(["init", "omnia", "--schema-dir"])
            .arg(repo_root())
            .args(["--name", "test-proj"])
            .assert()
            .success();
        copy_dir(&repo_root().join("schemas/omnia"), &root.join("schemas/omnia"));
        Project { _tmp: tmp, root }
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
    assert_eq!(value["schema_version"], 1);
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
