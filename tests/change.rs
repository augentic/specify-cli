//! Integration tests for the `specify change` subcommand tree.
//!
//! Every test stands up a fresh `.specify/` project via `specify init`,
//! drives `specify change *` through `assert_cmd`, and inspects both the
//! structured stdout (`--format json`) and the on-disk side effects the
//! verb is responsible for.
//!
//! Test style follows `tests/e2e.rs`: favour end-to-end execution of the
//! built binary over unit tests so the behaviour the skills consume is
//! the behaviour under test.

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
        Project { _tmp: tmp, root }
    }

    /// Copy the in-repo `schemas/omnia` tree into the project so any
    /// subcommand that loads a `PipelineView` can resolve the schema.
    /// Used by the list/status tests which walk the pipeline to report
    /// per-brief artifact completion.
    fn with_schemas(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join("schemas/omnia"));
        self
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn changes_dir(&self) -> PathBuf {
        self.root.join(".specify/changes")
    }

    fn specs_dir(&self) -> PathBuf {
        self.root.join(".specify/specs")
    }
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

// ---------------------------------------------------------------------------
// change create
// ---------------------------------------------------------------------------

#[test]
fn change_create_produces_directory_and_metadata() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "my-change"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["name"], "my-change");
    assert_eq!(value["status"], "defining");
    assert_eq!(value["schema"], "omnia");
    assert_eq!(value["created"], true);
    assert_eq!(value["restarted"], false);

    let change_dir = project.changes_dir().join("my-change");
    assert!(change_dir.is_dir(), "change dir must exist");
    assert!(change_dir.join("specs").is_dir(), "specs/ must exist");
    let meta = fs::read_to_string(change_dir.join(".metadata.yaml")).expect("read metadata");
    assert!(meta.contains("status: defining"));
    assert!(meta.contains("schema: omnia"));
    assert!(meta.contains("created-at:"));
}

#[test]
fn change_create_rejects_uppercase_name() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "BadName"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "config");
    assert!(value["message"].as_str().unwrap().contains("kebab-case"));
}

#[test]
fn change_create_fails_when_dir_exists_by_default() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "my-change"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "config");
    assert!(value["message"].as_str().unwrap().contains("already exists"));
}

#[test]
fn change_create_continue_reuses_existing_directory() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "my-change", "--if-exists", "continue"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["created"], false);
    assert_eq!(value["restarted"], false);
}

// ---------------------------------------------------------------------------
// change transition
// ---------------------------------------------------------------------------

#[test]
fn change_transition_walks_the_happy_path() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();

    for target in ["defined", "building", "complete"] {
        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "transition", "my-change", target])
            .assert()
            .success();
        let value = parse_json(&assert.get_output().stdout);
        assert_eq!(value["status"], target);
    }

    let meta = fs::read_to_string(project.changes_dir().join("my-change").join(".metadata.yaml"))
        .expect("read metadata");
    assert!(meta.contains("status: complete"));
    assert!(meta.contains("defined-at:"));
    assert!(meta.contains("build-started-at:"));
    assert!(meta.contains("completed-at:"));
}

#[test]
fn change_transition_rejects_illegal_edge() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();
    // Defining -> Building is not a legal edge.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "transition", "my-change", "building"])
        .assert()
        .failure();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "lifecycle");
}

// ---------------------------------------------------------------------------
// change touched-specs
// ---------------------------------------------------------------------------

#[test]
fn change_touched_specs_scan_classifies_new_vs_modified() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();
    let change_dir = project.changes_dir().join("my-change");

    // Capability `alpha` — no baseline, should classify as `new`.
    fs::create_dir_all(change_dir.join("specs/alpha")).unwrap();
    fs::write(change_dir.join("specs/alpha/spec.md"), "# Alpha\n").unwrap();

    // Capability `beta` — baseline exists, should classify as `modified`.
    fs::create_dir_all(project.specs_dir().join("beta")).unwrap();
    fs::write(project.specs_dir().join("beta/spec.md"), "# Beta baseline\n").unwrap();
    fs::create_dir_all(change_dir.join("specs/beta")).unwrap();
    fs::write(change_dir.join("specs/beta/spec.md"), "# Beta delta\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "touched-specs", "my-change", "--scan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["touched_specs"].as_array().expect("touched_specs array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[0]["type"], "new");
    assert_eq!(items[1]["name"], "beta");
    assert_eq!(items[1]["type"], "modified");

    // Scanning must have persisted the list into `.metadata.yaml`.
    let meta = fs::read_to_string(change_dir.join(".metadata.yaml")).unwrap();
    assert!(meta.contains("touched-specs:"));
    assert!(meta.contains("name: alpha"));
    assert!(meta.contains("type: new"));
    assert!(meta.contains("name: beta"));
    assert!(meta.contains("type: modified"));
}

#[test]
fn change_touched_specs_set_accepts_explicit_list() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "touched-specs",
            "my-change",
            "--set",
            "alpha:new,beta:modified",
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["touched_specs"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[1]["type"], "modified");
}

// ---------------------------------------------------------------------------
// change overlap
// ---------------------------------------------------------------------------

#[test]
fn change_overlap_reports_shared_capabilities() {
    let project = Project::init();
    // Two active changes both claim `login`.
    specify().current_dir(project.root()).args(["change", "create", "first"]).assert().success();
    specify().current_dir(project.root()).args(["change", "create", "second"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["change", "touched-specs", "first", "--set", "login:new,oauth:new"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args(["change", "touched-specs", "second", "--set", "login:modified"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "overlap", "first"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let overlaps = value["overlaps"].as_array().unwrap();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0]["capability"], "login");
    assert_eq!(overlaps[0]["other_change"], "second");
    assert_eq!(overlaps[0]["our_spec_type"], "new");
    assert_eq!(overlaps[0]["other_spec_type"], "modified");
}

#[test]
fn change_overlap_empty_for_disjoint_changes() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["change", "create", "alpha"]).assert().success();
    specify().current_dir(project.root()).args(["change", "create", "beta"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["change", "touched-specs", "alpha", "--set", "aa:new"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args(["change", "touched-specs", "beta", "--set", "bb:new"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "overlap", "alpha"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["overlaps"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// change archive and drop
// ---------------------------------------------------------------------------

#[test]
fn change_archive_moves_dir_into_dated_archive() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "archive", "my-change"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let archive_path = value["archive_path"].as_str().unwrap();
    assert!(archive_path.contains(".specify/archive/"));
    assert!(archive_path.ends_with("-my-change"));

    // Original is gone; archive dir has one dated subdirectory.
    assert!(!project.changes_dir().join("my-change").exists());
    let archive = project.root().join(".specify/archive");
    let entries: Vec<_> = fs::read_dir(&archive).unwrap().filter_map(|e| e.ok()).collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].file_name().to_string_lossy().ends_with("-my-change"));
}

#[test]
fn change_drop_transitions_and_archives_with_reason() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "my-change"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "drop",
            "my-change",
            "--reason",
            "Needs design call-out",
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["status"], "dropped");
    assert_eq!(value["drop_reason"], "Needs design call-out");
    let archive_path = value["archive_path"].as_str().unwrap();
    assert!(archive_path.ends_with("-my-change"));

    // `.metadata.yaml` inside the archive should reflect the drop.
    let archived_meta = fs::read_to_string(format!("{archive_path}/.metadata.yaml")).unwrap();
    assert!(archived_meta.contains("status: dropped"));
    assert!(archived_meta.contains("drop-reason: Needs design call-out"));
    assert!(archived_meta.contains("dropped-at:"));
}

// ---------------------------------------------------------------------------
// change list / status
// ---------------------------------------------------------------------------

#[test]
fn change_list_shows_every_active_change() {
    let project = Project::init().with_schemas();
    specify().current_dir(project.root()).args(["change", "create", "alpha"]).assert().success();
    specify().current_dir(project.root()).args(["change", "create", "beta"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "list"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let names: Vec<_> = value["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["alpha", "beta"]);
}

#[test]
fn change_status_by_name_returns_single_entry() {
    let project = Project::init().with_schemas();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "only-change"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "status", "only-change"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["changes"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "only-change");
    assert_eq!(items[0]["status"], "defining");
}
