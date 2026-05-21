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

mod common;
use common::{Project, parse_json, specify};

// ---------------------------------------------------------------------------
// slice create
// ---------------------------------------------------------------------------

#[test]
fn create_writes_dir_and_metadata() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    let dir = value["dir"].as_str().expect("dir string");
    assert!(dir.ends_with("/my-slice"), "dir should end with /my-slice, got: {dir}");
    assert_eq!(value["status"], "defining");
    let target = value["target"].as_str().expect("target string");
    assert!(target.starts_with("file://"));
    assert!(target.ends_with("/schemas/omnia"));
    assert_eq!(value["created"], true);
    assert_eq!(value["restarted"], false);

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(slice_dir.is_dir(), "slice dir must exist");
    assert!(slice_dir.join("specs").is_dir(), "specs/ must exist");
    let meta = fs::read_to_string(slice_dir.join(".metadata.yaml")).expect("read metadata");
    assert!(meta.contains("status: defining"));
    assert!(meta.contains("target: file://"));
    assert!(meta.contains("created-at:"));
}

#[test]
fn create_rejects_uppercase_name() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "BadName"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "invalid-name");
    assert!(
        value["message"].as_str().unwrap().contains("kebab-case")
            || value["message"].as_str().unwrap().contains("invalid name")
    );
}

#[test]
fn create_errors_on_collision() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-already-exists");
    assert!(value["message"].as_str().unwrap().contains("already exists"));
}

#[test]
fn create_continue_reuses_existing_dir() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
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
fn transition_walks_happy_path() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

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
fn transition_rejects_illegal_edge() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    // Defining -> Building is not a legal edge.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "transition", "my-slice", "building"])
        .assert()
        .failure();
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "lifecycle");
}

#[test]
fn transition_rejects_merged_target() {
    // The `merged` lifecycle status is reserved for `slice merge run`,
    // which writes it atomically alongside the spec merge and archive
    // move. Hand-driven `slice transition <name> merged` would skip
    // that bookkeeping, so the dispatcher refuses the value with an
    // argument-error envelope (exit 2) before lifecycle ever runs.
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "transition", "my-slice", "merged"])
        .assert()
        .code(2);
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "argument");
    assert_eq!(value["exit-code"], 2);
    let message = value["message"].as_str().expect("message string");
    assert!(
        message.contains("specify slice merge run"),
        "argument-error message must redirect to the merge runner; got:\n{message}"
    );
    assert!(
        message.contains("merged"),
        "argument-error message must name the rejected target; got:\n{message}"
    );
}

// ---------------------------------------------------------------------------
// slice touched-specs
// ---------------------------------------------------------------------------

#[test]
fn touched_specs_classifies_new_vs_modified() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");

    // Adapter `alpha` — no baseline, should classify as `new`.
    fs::create_dir_all(slice_dir.join("specs/alpha")).unwrap();
    fs::write(slice_dir.join("specs/alpha/spec.md"), "# Alpha\n").unwrap();

    // Adapter `beta` — baseline exists, should classify as `modified`.
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
fn touched_specs_accepts_explicit_list() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

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
fn overlap_reports_shared_adapters() {
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
fn overlap_empty_for_disjoint_slices() {
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
// slice drop
// ---------------------------------------------------------------------------

#[test]
fn drop_transitions_and_archives() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

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
// slice status
// ---------------------------------------------------------------------------

#[test]
fn status_by_name_returns_single_entry() {
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
    let entry = &value["slice"];
    assert_eq!(entry["name"], "only-slice");
    assert_eq!(entry["status"], "defining");
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
fn phase_outcome_stamps_success_on_define() {
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
fn phase_outcome_stamps_failure_with_context() {
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
fn phase_outcome_stamps_deferred_on_build() {
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
fn phase_outcome_text_output() {
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
fn phase_outcome_errors_on_missing_slice() {
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
    let value = parse_json(&assert.get_output().stderr);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

#[test]
fn phase_outcome_writes_trailing_newline() {
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
fn phase_outcome_overwrites_previous() {
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
fn phase_outcome_preserves_metadata_fields() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let meta_before = read_metadata_yaml(&project, "foo");
    let created_at_before =
        meta_before["created-at"].as_str().expect("created-at populated after create").to_string();
    let status_before =
        meta_before["status"].as_str().expect("status populated after create").to_string();
    let target_before =
        meta_before["target"].as_str().expect("target populated after create").to_string();

    specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "set", "foo", "define", "success", "--summary", "ok"])
        .assert()
        .success();

    let meta_after = read_metadata_yaml(&project, "foo");
    assert_eq!(meta_after["created-at"].as_str(), Some(created_at_before.as_str()));
    assert_eq!(meta_after["status"].as_str(), Some(status_before.as_str()));
    assert_eq!(meta_after["target"].as_str(), Some(target_before.as_str()));
    assert!(meta_after["outcome"].is_object(), "outcome should now be present");
}

#[test]
fn metadata_without_outcome_still_parses() {
    use specify_domain::slice::SliceMetadata;
    // Hand-craft a `.metadata.yaml` that predates the `outcome` field
    // and assert that SliceMetadata::load accepts it and leaves
    // `outcome` as None.
    let tmp = tempfile::tempdir().expect("tempdir");
    let slice_dir = tmp.path();
    let yaml = r#"target: omnia
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
fn outcome_returns_stamped_as_json() {
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
fn outcome_emits_null_when_unstamped() {
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
fn outcome_null_context_when_unstamped() {
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
fn outcome_text_output_stamped() {
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
fn outcome_text_output_unstamped() {
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
fn outcome_errors_on_missing_slice() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "ghost"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

#[test]
fn outcome_falls_back_to_archive() {
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
fn outcome_archive_picks_most_recent() {
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
            "target: omnia\nstatus: merged\ncreated-at: \"{created_at}\"\noutcome:\n  phase: merge\n  outcome: success\n  at: \"{created_at}\"\n  summary: \"{summary}\"\n"
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
fn outcome_registry_amendment_writes_payload() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let proposal = serde_json::json!({
        "proposed-name": "alpha-gateway",
        "proposed-url": "git@github.com:augentic/alpha-gateway.git",
        "proposed-adapter": "omnia@v1",
        "proposed-description": "Gateway for alpha adapter.",
        "rationale": "build discovered tangled code requiring a split",
    })
    .to_string();
    specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "registry-amendment-required",
            "--proposal",
            &proposal,
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
        raw.contains("proposed-adapter: \"omnia@v1\"")
            || raw.contains("proposed-adapter: omnia@v1"),
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
    let payload = &outcome["outcome"]["registry-amendment-required"];
    assert!(payload.is_object(), "expected externally-tagged variant, got: {outcome}");
    assert_eq!(payload["proposed-name"].as_str(), Some("alpha-gateway"));
    assert_eq!(payload["proposed-url"].as_str(), Some("git@github.com:augentic/alpha-gateway.git"),);
    assert_eq!(payload["proposed-adapter"].as_str(), Some("omnia@v1"));
    assert_eq!(payload["proposed-description"].as_str(), Some("Gateway for alpha adapter."));
    assert_eq!(
        payload["rationale"].as_str(),
        Some("build discovered tangled code requiring a split"),
    );
    assert_eq!(
        outcome["summary"].as_str(),
        Some("registry-amendment-required: alpha-gateway"),
        "missing --summary should default to `registry-amendment-required: <name>`",
    );
}

/// Missing required keys in the `--proposal` JSON object surface as a
/// clap parse error (exit `2`).
#[test]
fn outcome_registry_amendment_missing_keys() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let proposal = serde_json::json!({
        "proposed-name": "alpha-gateway",
        "proposed-url": "git@github.com:augentic/alpha-gateway.git",
    })
    .to_string();
    let assert = specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "registry-amendment-required",
            "--proposal",
            &proposal,
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("--proposal") && stderr.contains("missing field"),
        "expected clap parse-error naming --proposal and the missing field, got: {stderr}",
    );
}

/// Malformed JSON on `--proposal` is rejected at parse time (exit `2`).
#[test]
fn outcome_registry_amendment_malformed_json() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "registry-amendment-required",
            "--proposal",
            "not-json",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("--proposal"),
        "expected clap parse-error naming --proposal, got: {stderr}",
    );
}

/// Supplying `--proposal` with an outcome other than
/// `registry-amendment-required` is rejected — the flag is
/// outcome-scoped, and silently dropping it would mask author intent.
#[test]
fn outcome_proposal_flag_rejected_otherwise() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "slice",
            "outcome",
            "set",
            "foo",
            "build",
            "success",
            "--summary",
            "ok",
            "--proposal",
            "{}",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("--proposal") || stderr.contains("unexpected argument"),
        "expected clap diagnostic naming the offending flag, got: {stderr}",
    );
}

#[test]
fn phase_outcome_round_trips_serde() {
    use specify_domain::slice::Outcome;
    // Construction via struct literal would require crossing the
    // `#[non_exhaustive]` boundary on `Outcome`; round-trip through
    // YAML instead so the wire shape is what's exercised.
    for kind in ["success", "failure", "deferred"] {
        for phase in ["define", "build", "merge"] {
            let yaml = format!(
                "phase: {phase}\noutcome: {kind}\nat: \"2024-08-01T10:00:00Z\"\nsummary: some summary\n"
            );
            let parsed: Outcome = serde_saphyr::from_str(&yaml).expect("parse");
            let reserialised = serde_saphyr::to_string(&parsed).expect("serialize");
            let reparsed: Outcome = serde_saphyr::from_str(&reserialised).expect("reparse");
            assert_eq!(parsed, reparsed, "round-trip failed for yaml:\n{yaml}");
        }
    }
}

// ---- RFC-25 top-level help surfaces source/target axis verbs ----

#[test]
fn top_level_help_lists_source_and_target_axis_verbs() {
    let assert = specify().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("slice"), "RFC-25 --help must still list `slice`, got:\n{stdout}");
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("source ")),
        "RFC-25 --help must list the `source` axis verb, got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("target ")),
        "RFC-25 --help must list the `target` axis verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("change ")),
        "RFC-25 --help must NOT list the retired `change` verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("adapter ")),
        "RFC-25 --help must NOT list the retired `adapter` verb, got:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// RFC-25 §Requirement block contract — `slice validate` provenance gate
// ---------------------------------------------------------------------------

/// Stage a slice on disk and seed `<slice>/specs/login/spec.md`
/// directly, plus optionally a `plan.yaml` at the project root, so the
/// provenance gate inside `specify slice validate` has both the spec
/// file and a plan-level source-bindings context to cross-validate
/// against. Returns the project handle so the caller can drive
/// `specify slice validate` on it.
fn stage_slice_with_spec(spec_md: &str, plan_yaml: Option<&str>) -> Project {
    let project = Project::init().with_schemas();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let specs_dir = project.slices_dir().join("my-slice/specs/login");
    fs::create_dir_all(&specs_dir).expect("mkdir specs/login");
    fs::write(specs_dir.join("spec.md"), spec_md).expect("write spec.md");
    if let Some(yaml) = plan_yaml {
        project.seed_plan(yaml);
    }
    project
}

/// Validate-fail goldens carry a `validation` discriminant; assert
/// that the wire envelope holds the expected `rule_id` exactly once.
fn assert_provenance_fail_rule(stderr: &[u8], rule_id: &str) {
    let value = parse_json(stderr);
    assert_eq!(value["error"], "validation", "wire envelope must be `validation`");
    assert_eq!(value["exit-code"], 2);
    let results = value["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["rule-id"] == rule_id),
        "expected rule_id `{rule_id}` in results: {results:#?}"
    );
}

const PLAN_WITH_LEGACY_MONOLITH: &str = "\
name: rfc25-prov
lifecycle: pending
sources:
  legacy-monolith: ./legacy
slices:
  - name: my-slice
    status: pending
    sources:
      - { key: legacy-monolith, candidate: my-slice }
";

#[test]
fn validate_rejects_missing_id_with_exit_two() {
    let spec = "### Requirement: Missing id\n\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n\n\
                body\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-id-missing");
}

#[test]
fn validate_rejects_malformed_id_with_exit_two() {
    let spec = "### Requirement: Malformed id\n\n\
                ID: REQ-1\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-id-malformed");
}

#[test]
fn validate_rejects_missing_sources_with_exit_two() {
    let spec = "### Requirement: No sources\n\n\
                ID: REQ-001\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-sources-missing");
}

#[test]
fn validate_rejects_missing_status_with_exit_two() {
    let spec = "### Requirement: No status\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-status-missing");
}

#[test]
fn validate_rejects_unknown_status_value_with_exit_two() {
    let spec = "### Requirement: Bogus status\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n\
                Status: maybe\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(
        &assert.get_output().stderr,
        "spec.requirement-status-unknown-value",
    );
}

#[test]
fn validate_rejects_source_key_not_in_plan_with_exit_two() {
    let spec = "### Requirement: Stray source key\n\n\
                ID: REQ-001\n\
                Sources: [phantom]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(
        &assert.get_output().stderr,
        "spec.requirement-source-key-undefined",
    );
}

#[test]
fn validate_rejects_tag_status_mismatch_with_exit_two() {
    let spec = "### Requirement: Lying tag [divergence]\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(
        &assert.get_output().stderr,
        "spec.requirement-tag-status-mismatch",
    );
}

#[test]
fn validate_skips_provenance_when_no_metadata_lines_present() {
    // Pre-RFC-25 (or pre-synthesis) state. The provenance gate must
    // not fire and the slice progresses to the existing adapter rule
    // run. The adapter rules will still surface deferred /
    // pass-style results — we only assert the provenance rule ids
    // are NOT present.
    let spec = "### Requirement: Pre-RFC-25 body\n\n\
                ID: REQ-001\n\n\
                body that has no Sources or Status yet\n";
    let project = stage_slice_with_spec(spec, None);
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let stderr = assert.get_output().stderr.clone();
    // Whether the run passes or fails (existing adapter rules may
    // still produce findings on the synthetic slice), no provenance
    // rule should appear.
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stderr)
        && let Some(results) = value["results"].as_array()
    {
        for r in results {
            let rule_id = r["rule-id"].as_str().unwrap_or("");
            assert!(
                !rule_id.starts_with("spec.requirement-"),
                "no provenance rule should fire on a pre-RFC-25 spec.md, got: {rule_id}"
            );
        }
    }
}
