//! Integration tests for the `specify slice` subcommand tree.
//!
//! Every test stands up a fresh `.specify/` project via `specify init`,
//! drives `specify slice *` through `assert_cmd`, and inspects both the
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
            .args(["init"])
            .arg(repo_root().join("schemas").join("omnia"))
            .args(["--name", "test-proj"])
            .assert()
            .success();
        Self { _tmp: tmp, root }
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

    fn slices_dir(&self) -> PathBuf {
        self.root.join(".specify/slices")
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
// slice create
// ---------------------------------------------------------------------------

#[test]
fn slice_create_produces_directory_and_metadata() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["name"], "my-slice");
    assert_eq!(value["status"], "defining");
    let schema = value["schema"].as_str().expect("schema string");
    assert!(schema.starts_with("file://"));
    assert!(schema.ends_with("/schemas/omnia"));
    assert_eq!(value["created"], true);
    assert_eq!(value["restarted"], false);

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(slice_dir.is_dir(), "slice dir must exist");
    assert!(slice_dir.join("specs").is_dir(), "specs/ must exist");
    let meta = fs::read_to_string(slice_dir.join(".metadata.yaml")).expect("read metadata");
    assert!(meta.contains("status: defining"));
    assert!(meta.contains("schema: file://"));
    assert!(meta.contains("created-at:"));
}

#[test]
fn slice_create_rejects_uppercase_name() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "BadName"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "invalid-name");
    assert!(
        value["message"].as_str().unwrap().contains("kebab-case")
            || value["message"].as_str().unwrap().contains("invalid name")
    );
}

#[test]
fn slice_create_fails_when_dir_exists_by_default() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "config");
    assert!(value["message"].as_str().unwrap().contains("already exists"));
}

#[test]
fn slice_create_continue_reuses_existing_directory() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice", "--if-exists", "continue"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["created"], false);
    assert_eq!(value["restarted"], false);
}

// ---------------------------------------------------------------------------
// slice transition
// ---------------------------------------------------------------------------

#[test]
fn slice_transition_walks_the_happy_path() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();

    for target in ["defined", "building", "complete"] {
        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "slice", "transition", "my-slice", target])
            .assert()
            .success();
        let value = parse_json(&assert.get_output().stdout);
        assert_eq!(value["status"], target);
    }

    let meta = fs::read_to_string(project.slices_dir().join("my-slice").join(".metadata.yaml"))
        .expect("read metadata");
    assert!(meta.contains("status: complete"));
    assert!(meta.contains("defined-at:"));
    assert!(meta.contains("build-started-at:"));
    assert!(meta.contains("completed-at:"));
}

#[test]
fn slice_transition_rejects_illegal_edge() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    // Defining -> Building is not a legal edge.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "transition", "my-slice", "building"])
        .assert()
        .failure();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "lifecycle");
}

// ---------------------------------------------------------------------------
// slice touched-specs
// ---------------------------------------------------------------------------

#[test]
fn slice_touched_specs_scan_classifies_new_vs_modified() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    let slice_dir = project.slices_dir().join("my-slice");

    // Capability `alpha` — no baseline, should classify as `new`.
    fs::create_dir_all(slice_dir.join("specs/alpha")).unwrap();
    fs::write(slice_dir.join("specs/alpha/spec.md"), "# Alpha\n").unwrap();

    // Capability `beta` — baseline exists, should classify as `modified`.
    fs::create_dir_all(project.specs_dir().join("beta")).unwrap();
    fs::write(project.specs_dir().join("beta/spec.md"), "# Beta baseline\n").unwrap();
    fs::create_dir_all(slice_dir.join("specs/beta")).unwrap();
    fs::write(slice_dir.join("specs/beta/spec.md"), "# Beta delta\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "touched-specs", "my-slice", "--scan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["touched-specs"].as_array().expect("touched-specs array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[0]["type"], "new");
    assert_eq!(items[1]["name"], "beta");
    assert_eq!(items[1]["type"], "modified");

    // Scanning must have persisted the list into `.metadata.yaml`.
    let meta = fs::read_to_string(slice_dir.join(".metadata.yaml")).unwrap();
    assert!(meta.contains("touched-specs:"));
    assert!(meta.contains("name: alpha"));
    assert!(meta.contains("type: new"));
    assert!(meta.contains("name: beta"));
    assert!(meta.contains("type: modified"));
}

#[test]
fn slice_touched_specs_set_accepts_explicit_list() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "touched-specs",
            "my-slice",
            "--set",
            "alpha:new,beta:modified",
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["touched-specs"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[1]["type"], "modified");
}

// ---------------------------------------------------------------------------
// slice overlap
// ---------------------------------------------------------------------------

#[test]
fn slice_overlap_reports_shared_capabilities() {
    let project = Project::init();
    // Two active slices both claim `login`.
    specify().current_dir(project.root()).args(["slice", "create", "first"]).assert().success();
    specify().current_dir(project.root()).args(["slice", "create", "second"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "first", "--set", "login:new,oauth:new"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "second", "--set", "login:modified"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "overlap", "first"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let overlaps = value["overlaps"].as_array().unwrap();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0]["capability"], "login");
    assert_eq!(overlaps[0]["other-slice"], "second");
    assert_eq!(overlaps[0]["our-spec-type"], "new");
    assert_eq!(overlaps[0]["other-spec-type"], "modified");
}

#[test]
fn slice_overlap_empty_for_disjoint_slices() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "alpha"]).assert().success();
    specify().current_dir(project.root()).args(["slice", "create", "beta"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "alpha", "--set", "aa:new"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "beta", "--set", "bb:new"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "overlap", "alpha"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["overlaps"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// slice archive and drop
// ---------------------------------------------------------------------------

#[test]
fn slice_archive_moves_dir_into_dated_archive() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "archive", "my-slice"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let archive_path = value["archive-path"].as_str().unwrap();
    assert!(archive_path.contains(".specify/archive/"));
    assert!(archive_path.ends_with("-my-slice"));

    // Original is gone; archive dir has one dated subdirectory.
    assert!(!project.slices_dir().join("my-slice").exists());
    let archive = project.root().join(".specify/archive");
    let entries: Vec<_> =
        fs::read_dir(&archive).unwrap().filter_map(std::result::Result::ok).collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].file_name().to_string_lossy().ends_with("-my-slice"));
}

#[test]
fn slice_drop_transitions_and_archives_with_reason() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "drop",
            "my-slice",
            "--reason",
            "Needs design call-out",
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["status"], "dropped");
    assert_eq!(value["drop-reason"], "Needs design call-out");
    let archive_path = value["archive-path"].as_str().unwrap();
    assert!(archive_path.ends_with("-my-slice"));

    // `.metadata.yaml` inside the archive should reflect the drop.
    let archived_meta = fs::read_to_string(format!("{archive_path}/.metadata.yaml")).unwrap();
    assert!(archived_meta.contains("status: dropped"));
    assert!(archived_meta.contains("drop-reason: Needs design call-out"));
    assert!(archived_meta.contains("dropped-at:"));
}

// ---------------------------------------------------------------------------
// slice list / status
// ---------------------------------------------------------------------------

#[test]
fn slice_list_shows_every_active_slice() {
    let project = Project::init().with_schemas();
    specify().current_dir(project.root()).args(["slice", "create", "alpha"]).assert().success();
    specify().current_dir(project.root()).args(["slice", "create", "beta"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "list"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let names: Vec<_> = value["slices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["alpha", "beta"]);
}

#[test]
fn slice_status_by_name_returns_single_entry() {
    let project = Project::init().with_schemas();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "only-slice"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "status", "only-slice"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["slices"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "only-slice");
    assert_eq!(items[0]["status"], "defining");
}

// ---------------------------------------------------------------------------
// slice outcome set (L2.A)
// ---------------------------------------------------------------------------

/// Parse the `.metadata.yaml` for `name` under `project` as a
/// `serde_json::Value` so tests can assert on the `outcome` subtree
/// without pulling in the `specify-slice` crate directly.
fn read_metadata_yaml(project: &Project, name: &str) -> serde_json::Value {
    let path = project.slices_dir().join(name).join(".metadata.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_saphyr::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Naive RFC3339 sanity check sufficient for integration tests: `YYYY-MM-DDT...`.
fn looks_like_rfc3339(s: &str) -> bool {
    s.len() >= 20
        && s.chars().nth(4) == Some('-')
        && s.chars().nth(7) == Some('-')
        && s.chars().nth(10) == Some('T')
}

#[test]
fn slice_phase_outcome_stamps_success_on_define_json() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "outcome",
            "set",
            "foo",
            "define",
            "success",
            "--summary",
            "artifacts generated",
        ])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["slice"], "foo");
    assert_eq!(value["phase"], "define");
    assert_eq!(value["outcome"], "success");
    let at = value["at"].as_str().expect("at is a string");
    assert!(looks_like_rfc3339(at), "at should be RFC3339, got {at}");

    let meta = read_metadata_yaml(&project, "foo");
    let outcome = &meta["outcome"];
    assert_eq!(outcome["phase"].as_str(), Some("define"));
    assert_eq!(outcome["outcome"].as_str(), Some("success"));
    assert_eq!(outcome["summary"].as_str(), Some("artifacts generated"));
    let at_on_disk = outcome["at"].as_str().expect("at on disk");
    assert!(looks_like_rfc3339(at_on_disk), "on-disk at should be RFC3339, got {at_on_disk}");
    assert!(
        outcome.get("context").is_none_or(serde_json::Value::is_null),
        "context must be absent when not supplied, got: {outcome:?}"
    );
}

#[test]
fn slice_phase_outcome_stamps_failure_with_context() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "failure",
            "--summary",
            "build broke",
            "--context",
            "task 3 failed",
        ])
        .assert()
        .success();

    let meta = read_metadata_yaml(&project, "foo");
    assert_eq!(meta["outcome"]["phase"].as_str(), Some("build"));
    assert_eq!(meta["outcome"]["outcome"].as_str(), Some("failure"));
    assert_eq!(meta["outcome"]["context"].as_str(), Some("task 3 failed"));
}

#[test]
fn slice_phase_outcome_stamps_deferred_on_build() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "deferred",
            "--summary",
            "channel scope unclear",
        ])
        .assert()
        .success();

    let meta = read_metadata_yaml(&project, "foo");
    assert_eq!(meta["outcome"]["phase"].as_str(), Some("build"));
    assert_eq!(meta["outcome"]["outcome"].as_str(), Some("deferred"));
    assert_eq!(meta["outcome"]["summary"].as_str(), Some("channel scope unclear"));
}

#[test]
fn slice_phase_outcome_text_output() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "define", "success", "--summary", "ok"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    assert_eq!(stdout.trim_end(), "Stamped outcome 'success' for phase 'define' on slice 'foo'.");
}

#[test]
fn slice_phase_outcome_on_nonexistent_slice_errors() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "outcome",
            "set",
            "ghost",
            "define",
            "success",
            "--summary",
            "x",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

#[test]
fn slice_phase_outcome_writes_trailing_newline() {
    // Atomicity is an OS-level guarantee (NamedTempFile + rename) so it
    // is not directly unit-testable. Instead assert the saved file
    // shape: trailing newline, mirroring the Plan::save atomic-save
    // tests.
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "define", "success", "--summary", "ok"])
        .assert()
        .success();

    let path = project.slices_dir().join("foo").join(".metadata.yaml");
    let bytes = fs::read(&path).expect("read metadata");
    assert!(!bytes.is_empty(), "metadata should not be empty");
    assert_eq!(
        *bytes.last().unwrap(),
        b'\n',
        "metadata must end with a trailing newline after atomic stamp"
    );
}

#[test]
fn slice_phase_outcome_overwrites_previous_outcome() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "define", "success", "--summary", "defined"])
        .assert()
        .success();

    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "failure",
            "--summary",
            "broke",
            "--context",
            "stderr blob",
        ])
        .assert()
        .success();

    let meta = read_metadata_yaml(&project, "foo");
    let outcome = &meta["outcome"];
    assert_eq!(outcome["phase"].as_str(), Some("build"));
    assert_eq!(outcome["outcome"].as_str(), Some("failure"));
    assert_eq!(outcome["summary"].as_str(), Some("broke"));
    assert_eq!(outcome["context"].as_str(), Some("stderr blob"));

    // Document that outcome is a single field, not a list: the raw
    // YAML text must contain exactly one top-level `outcome:` key.
    let path = project.slices_dir().join("foo").join(".metadata.yaml");
    let text = fs::read_to_string(&path).expect("read metadata");
    let outcome_lines = text.lines().filter(|l| l.starts_with("outcome:")).count();
    assert_eq!(
        outcome_lines, 1,
        "expected exactly one top-level `outcome:` key, got {outcome_lines} in:\n{text}"
    );
}

#[test]
fn slice_phase_outcome_preserves_existing_metadata_fields() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let meta_before = read_metadata_yaml(&project, "foo");
    let created_at_before =
        meta_before["created-at"].as_str().expect("created-at populated after create").to_string();
    let status_before =
        meta_before["status"].as_str().expect("status populated after create").to_string();
    let schema_before =
        meta_before["schema"].as_str().expect("schema populated after create").to_string();

    specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "define", "success", "--summary", "ok"])
        .assert()
        .success();

    let meta_after = read_metadata_yaml(&project, "foo");
    assert_eq!(meta_after["created-at"].as_str(), Some(created_at_before.as_str()));
    assert_eq!(meta_after["status"].as_str(), Some(status_before.as_str()));
    assert_eq!(meta_after["schema"].as_str(), Some(schema_before.as_str()));
    assert!(meta_after["outcome"].is_object(), "outcome should now be present");
}

#[test]
fn pre_existing_metadata_yaml_without_outcome_still_parses() {
    use specify::SliceMetadata;
    // Hand-craft a `.metadata.yaml` that predates the `outcome` field
    // and assert that SliceMetadata::load accepts it and leaves
    // `outcome` as None.
    let tmp = tempdir().expect("tempdir");
    let slice_dir = tmp.path();
    let yaml = r#"schema: omnia
status: defining
created-at: "2024-08-01T10:00:00Z"
"#;
    fs::write(slice_dir.join(".metadata.yaml"), yaml).expect("write metadata");
    let meta = SliceMetadata::load(slice_dir).expect("legacy metadata parses");
    assert!(
        meta.outcome.is_none(),
        "pre-existing metadata without an outcome field must load as None"
    );
}

// ---------------------------------------------------------------------------
// slice outcome show (read verb symmetric with `outcome set`)
// ---------------------------------------------------------------------------

#[test]
fn slice_outcome_returns_stamped_outcome_as_json() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();
    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "success",
            "--summary",
            "5/5 tasks",
            "--context",
            "trailing newline",
        ])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["name"], "foo");
    let outcome = &value["outcome"];
    assert_eq!(outcome["phase"].as_str(), Some("build"));
    assert_eq!(outcome["outcome"].as_str(), Some("success"));
    assert_eq!(outcome["summary"].as_str(), Some("5/5 tasks"));
    assert_eq!(outcome["context"].as_str(), Some("trailing newline"));
    let at = outcome["at"].as_str().expect("at is a string");
    assert!(looks_like_rfc3339(at), "at should be RFC3339, got {at}");
}

#[test]
fn slice_outcome_emits_null_when_unstamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["name"], "foo");
    assert!(
        value["outcome"].is_null(),
        "outcome must be null when not yet stamped, got: {}",
        value["outcome"]
    );
    assert_eq!(assert.get_output().status.code(), Some(0));
}

#[test]
fn slice_outcome_null_context_when_stamped_without_context() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "define", "success", "--summary", "ok"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    let outcome = &value["outcome"];
    assert!(
        outcome["context"].is_null(),
        "context must render as null when absent, got: {}",
        outcome["context"]
    );
}

#[test]
fn slice_outcome_text_output_stamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "build", "success", "--summary", "5/5 tasks"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "show", "foo"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    assert_eq!(stdout.trim_end(), "foo: build/success — 5/5 tasks");
}

#[test]
fn slice_outcome_text_output_unstamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "show", "foo"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    assert_eq!(stdout.trim_end(), "foo: no outcome stamped");
}

#[test]
fn slice_outcome_on_nonexistent_slice_errors() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "ghost"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

#[test]
fn slice_outcome_falls_back_to_archive() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "bar"]).assert().success();
    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "bar",
            "merge",
            "success",
            "--summary",
            "Merged 2 spec(s) into baseline",
        ])
        .assert()
        .success();

    // Simulate the archive move that `specify merge` performs.
    let slices_dir = project.root().join(".specify/slices");
    let archive_dir = project.root().join(".specify/archive");
    fs::create_dir_all(&archive_dir).unwrap();
    fs::rename(slices_dir.join("bar"), archive_dir.join("2026-04-24-bar")).unwrap();

    // The active slice directory is gone; outcome should resolve from archive.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "bar"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["name"], "bar");
    let outcome = &value["outcome"];
    assert_eq!(outcome["phase"].as_str(), Some("merge"));
    assert_eq!(outcome["outcome"].as_str(), Some("success"));
    assert_eq!(outcome["summary"].as_str(), Some("Merged 2 spec(s) into baseline"));
}

#[test]
fn slice_outcome_archive_fallback_picks_most_recent() {
    let project = Project::init();

    // Create and stamp two archived versions with different created-at timestamps.
    let archive_dir = project.root().join(".specify/archive");
    fs::create_dir_all(&archive_dir).unwrap();

    for (date, summary) in [("2026-01-01-baz", "old run"), ("2026-04-24-baz", "latest run")] {
        let dir = archive_dir.join(date);
        fs::create_dir_all(&dir).unwrap();
        let created_at = if date.starts_with("2026-01") {
            "2026-01-01T00:00:00Z"
        } else {
            "2026-04-24T00:00:00Z"
        };
        let yaml = format!(
            "schema: omnia\nstatus: merged\ncreated-at: \"{created_at}\"\noutcome:\n  phase: merge\n  outcome: success\n  at: \"{created_at}\"\n  summary: \"{summary}\"\n"
        );
        fs::write(dir.join(".metadata.yaml"), yaml).unwrap();
    }

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "baz"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(
        value["outcome"]["summary"].as_str(),
        Some("latest run"),
        "should pick the most recent archive entry"
    );
}

// ---------------------------------------------------------------------------
// slice outcome set — registry-amendment-required (RFC-9 §2B)
// ---------------------------------------------------------------------------

/// Stamping the new outcome variant writes the structured proposal
/// payload to `.metadata.yaml` under `outcome.outcome.registry-amendment-required.*`
/// (kebab-case external-tag form). Round-trips through the writer.
#[test]
fn slice_outcome_registry_amendment_required_writes_payload() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "registry-amendment-required",
            "--proposed-name",
            "alpha-gateway",
            "--proposed-url",
            "git@github.com:augentic/alpha-gateway.git",
            "--proposed-schema",
            "omnia@v1",
            "--proposed-description",
            "Gateway for alpha capability.",
            "--rationale",
            "build discovered tangled code requiring a split",
        ])
        .assert()
        .success();

    let path = project.slices_dir().join("foo").join(".metadata.yaml");
    let raw = fs::read_to_string(&path).expect("read metadata");
    assert!(
        raw.contains("registry-amendment-required:"),
        "outcome should use external-tag form, got:\n{raw}"
    );
    assert!(
        raw.contains("proposed-name: alpha-gateway"),
        "proposal fields should be kebab-case, got:\n{raw}"
    );
    assert!(
        raw.contains("proposed-url: \"git@github.com:augentic/alpha-gateway.git\"")
            || raw.contains("proposed-url: git@github.com:augentic/alpha-gateway.git"),
        "proposed-url should round-trip the verbatim URL, got:\n{raw}"
    );
    assert!(
        raw.contains("proposed-schema: \"omnia@v1\"") || raw.contains("proposed-schema: omnia@v1"),
        "proposed-schema should round-trip, got:\n{raw}"
    );
    assert!(raw.contains("rationale:"), "rationale should be emitted, got:\n{raw}");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let outcome = &value["outcome"];
    assert_eq!(outcome["outcome"].as_str(), Some("registry-amendment-required"));
    let proposal = &outcome["proposal"];
    assert_eq!(proposal["proposed-name"].as_str(), Some("alpha-gateway"));
    assert_eq!(
        proposal["proposed-url"].as_str(),
        Some("git@github.com:augentic/alpha-gateway.git"),
    );
    assert_eq!(proposal["proposed-schema"].as_str(), Some("omnia@v1"));
    assert_eq!(proposal["proposed-description"].as_str(), Some("Gateway for alpha capability."),);
    assert_eq!(
        proposal["rationale"].as_str(),
        Some("build discovered tangled code requiring a split"),
    );
    assert_eq!(
        outcome["summary"].as_str(),
        Some("registry-amendment-required: alpha-gateway"),
        "missing --summary should default to `registry-amendment-required: <name>`",
    );
}

/// Missing required flags surface a clear `Error::Config` (exit code 1).
#[test]
fn slice_outcome_registry_amendment_required_missing_flags_errors() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "registry-amendment-required",
            "--proposed-name",
            "alpha-gateway",
            "--proposed-url",
            "git@github.com:augentic/alpha-gateway.git",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("--proposed-schema") || msg.contains("--rationale"),
        "expected diagnostic naming the missing required flag, got: {msg}",
    );
}

/// Supplying `--proposed-*` flags with an outcome other than
/// `registry-amendment-required` is rejected — those flags are
/// outcome-scoped, and silently dropping them would mask author intent.
#[test]
fn slice_outcome_proposal_flags_rejected_for_other_kinds() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "success",
            "--summary",
            "ok",
            "--proposed-name",
            "alpha",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("--proposed-name"),
        "expected diagnostic naming the offending flag, got: {msg}",
    );
}

// ---------------------------------------------------------------------------
// slice journal append (L2.B)
// ---------------------------------------------------------------------------

#[test]
fn slice_journal_append_appends_to_file() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "journal",
            "append",
            "foo",
            "define",
            "question",
            "--summary",
            "scope unclear",
            "--context",
            "line one\nline two",
        ])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["slice"], "foo");
    assert_eq!(value["phase"], "define");
    assert_eq!(value["kind"], "question");

    let journal_path = project.slices_dir().join("foo").join("journal.yaml");
    assert!(journal_path.is_file(), "journal.yaml must exist after append");
    let text = fs::read_to_string(&journal_path).expect("read journal");
    assert!(text.contains("entries:"), "missing entries list in:\n{text}");
    assert!(text.contains("step: define"), "missing kebab-case step:\n{text}");
    assert!(text.contains("type: question"), "missing literal `type: question`:\n{text}");
    assert!(text.contains("summary: scope unclear"), "missing summary:\n{text}");
    assert!(text.contains("line one"), "missing first context line:\n{text}");
    assert!(text.contains("line two"), "missing second context line:\n{text}");
    assert_eq!(
        *text.as_bytes().last().unwrap(),
        b'\n',
        "journal.yaml must end with a trailing newline"
    );

    let yaml: serde_json::Value = serde_saphyr::from_str(&text).expect("parse journal");
    let entries = yaml["entries"].as_array().expect("entries seq");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["step"].as_str(), Some("define"));
    assert_eq!(entries[0]["type"].as_str(), Some("question"));
    assert_eq!(entries[0]["summary"].as_str(), Some("scope unclear"));
    assert_eq!(entries[0]["context"].as_str(), Some("line one\nline two"));
}

#[test]
fn slice_journal_append_stamps_rfc3339_timestamp() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "journal",
            "append",
            "foo",
            "build",
            "failure",
            "--summary",
            "task 3 failed",
        ])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    let stamp = value["timestamp"].as_str().expect("timestamp string");
    assert!(looks_like_rfc3339(stamp), "CLI-reported timestamp should be RFC3339, got {stamp}");

    // `chrono::DateTime::parse_from_rfc3339` is the authoritative check.
    chrono::DateTime::parse_from_rfc3339(stamp)
        .unwrap_or_else(|e| panic!("CLI timestamp {stamp} is not valid RFC3339: {e}"));

    let journal_path = project.slices_dir().join("foo").join("journal.yaml");
    let text = fs::read_to_string(&journal_path).expect("read journal");
    let yaml: serde_json::Value = serde_saphyr::from_str(&text).expect("parse journal");
    let on_disk = yaml["entries"][0]["timestamp"].as_str().expect("timestamp on disk");
    chrono::DateTime::parse_from_rfc3339(on_disk)
        .unwrap_or_else(|e| panic!("on-disk timestamp {on_disk} is not valid RFC3339: {e}"));
    assert_eq!(on_disk, stamp, "on-disk timestamp must match the JSON payload");
}

#[test]
fn slice_journal_append_preserves_existing_entries() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    for (phase, kind, summary) in [
        ("define", "question", "first"),
        ("build", "failure", "second"),
        ("build", "recovery", "third"),
    ] {
        specify()
            .current_dir(project.root())
            .args(["slice", "journal", "append", "foo", phase, kind, "--summary", summary])
            .assert()
            .success();
    }

    let text =
        fs::read_to_string(project.slices_dir().join("foo").join("journal.yaml")).expect("read");
    let yaml: serde_json::Value = serde_saphyr::from_str(&text).expect("parse");
    let entries = yaml["entries"].as_array().expect("entries seq");
    assert_eq!(entries.len(), 3, "all three appends must persist");
    let summaries: Vec<&str> =
        entries.iter().map(|e| e["summary"].as_str().expect("summary")).collect();
    assert_eq!(summaries, vec!["first", "second", "third"]);
}

#[test]
fn slice_journal_append_text_output() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "journal", "append", "foo", "define", "question", "--summary", "why"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    assert_eq!(stdout.trim_end(), "Appended question entry to foo/journal.yaml.");
}

#[test]
fn slice_journal_append_on_nonexistent_slice_errors() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "journal",
            "append",
            "ghost",
            "define",
            "question",
            "--summary",
            "x",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

// ---------------------------------------------------------------------------
// slice journal show
// ---------------------------------------------------------------------------

#[test]
fn slice_journal_show_empty_then_populated() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    // Empty journal — show must return an empty entries array.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "journal", "show", "foo"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["name"], "foo");
    assert!(
        value["entries"].as_array().unwrap().is_empty(),
        "expected empty entries on a fresh slice, got: {}",
        value["entries"]
    );

    // Text mode for the empty case: the per-slice "no journal entries" line.
    let text = specify()
        .current_dir(project.root())
        .args(["slice", "journal", "show", "foo"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).unwrap();
    assert!(
        stdout.contains("foo: no journal entries"),
        "text show on empty journal should announce no entries, got: {stdout:?}"
    );

    // Append two entries and verify show reports them in order.
    specify()
        .current_dir(project.root())
        .args(["slice", "journal", "append", "foo", "define", "question", "--summary", "first"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "journal",
            "append",
            "foo",
            "build",
            "failure",
            "--summary",
            "second",
            "--context",
            "stderr blob",
        ])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "journal", "show", "foo"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let entries = value["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["phase"], "define");
    assert_eq!(entries[0]["kind"], "question");
    assert_eq!(entries[0]["summary"], "first");
    assert!(entries[0]["context"].is_null());
    assert_eq!(entries[1]["phase"], "build");
    assert_eq!(entries[1]["kind"], "failure");
    assert_eq!(entries[1]["summary"], "second");
    assert_eq!(entries[1]["context"], "stderr blob");
}

#[test]
fn slice_journal_show_on_nonexistent_slice_errors() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "journal", "show", "ghost"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

#[test]
fn phase_outcome_round_trips_through_serde() {
    use specify::{Outcome, Phase, PhaseOutcome, Rfc3339Stamp};
    for outcome in [Outcome::Success, Outcome::Failure, Outcome::Deferred] {
        for phase in [Phase::Define, Phase::Build, Phase::Merge] {
            let context = if matches!(outcome, Outcome::Success) {
                None
            } else {
                Some("verbatim detail".to_string())
            };
            let value = PhaseOutcome {
                phase,
                outcome: outcome.clone(),
                at: Rfc3339Stamp::from_raw("2024-08-01T10:00:00+00:00".to_string()),
                summary: "some summary".to_string(),
                context,
            };
            let yaml = serde_saphyr::to_string(&value).expect("serialize");
            let parsed: PhaseOutcome = serde_saphyr::from_str(&yaml).expect("parse");
            assert_eq!(parsed, value, "round-trip failed for yaml:\n{yaml}");
        }
    }
}

// ---- specify {initiative, plan} * are gone (RFC-13 Phase 3.5 cut-over) ----
//
// Phase 3.2 deleted the pre-RFC `change` per-loop-unit verb (renamed to
// `slice`). Phase 3.5 then re-uses the `change` noun as the operator-
// facing umbrella that nests the plan sub-resource: `Commands::Change`
// folds in what used to be `Commands::Initiative` and adds a nested
// `change plan *` family that replaces top-level `Commands::Plan`. The
// regression tests below pin the post-3.5 surface — `change` is back as
// a top-level subcommand, while `initiative` and `plan` are gone from
// the binary's `--help` and trip clap's unrecognised-subcommand path.

#[test]
fn change_umbrella_is_listed_in_top_level_help() {
    let assert = specify().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        stdout.contains("slice"),
        "post-RFC-13 --help must list `slice`, got:\n{stdout}"
    );
    // `--help` lists subcommand names one-per-line (clap default).
    // After Phase 3.5, the umbrella `change` subcommand is back.
    assert!(
        stdout
            .lines()
            .any(|line| line.trim_start().starts_with("change ")),
        "post-3.5 --help must list `change` as the umbrella subcommand, got:\n{stdout}"
    );
}

#[test]
fn initiative_subcommand_is_gone_from_top_level_help() {
    let assert = specify().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        !stdout.lines().any(|line| {
            line.trim_start().starts_with("initiative ")
                || line.trim_start() == "initiative"
        }),
        "post-3.5 --help must not list `initiative` (folded into `change`), got:\n{stdout}"
    );
}

#[test]
fn plan_subcommand_is_gone_from_top_level_help() {
    let assert = specify().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        !stdout
            .lines()
            .any(|line| line.trim_start().starts_with("plan ") || line.trim_start() == "plan"),
        "post-3.5 --help must not list top-level `plan` (folded into `change plan`), got:\n{stdout}"
    );
}

#[test]
fn initiative_subcommand_returns_clap_unrecognised_subcommand_error() {
    let assert = specify().args(["initiative", "create", "demo"]).assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8 stderr");
    assert!(
        stderr.to_lowercase().contains("unrecognized")
            || stderr.to_lowercase().contains("unrecognised")
            || stderr.to_lowercase().contains("unexpected argument"),
        "post-3.5 `specify initiative *` must be a clap-level error, got stderr:\n{stderr}"
    );
}

#[test]
fn plan_subcommand_returns_clap_unrecognised_subcommand_error() {
    let assert = specify().args(["plan", "add", "foo"]).assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8 stderr");
    assert!(
        stderr.to_lowercase().contains("unrecognized")
            || stderr.to_lowercase().contains("unrecognised")
            || stderr.to_lowercase().contains("unexpected argument"),
        "post-3.5 top-level `specify plan *` must be a clap-level error, got stderr:\n{stderr}"
    );
}
