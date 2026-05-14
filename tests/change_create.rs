//! Integration tests for `specify change create` — scaffolds the
//! operator brief at `change.md` (at the repo root). Template byte
//! stability is the key contract: `create` must produce the same bytes
//! every time so operators can diff against the RFC-matching golden.

use std::fs;
use std::path::PathBuf;

mod common;
use common::{Project, parse_stderr, parse_stdout, specify};

/// Byte-for-byte golden for `specify change create
/// traffic-modernisation`. Kept in-source (not a fixture file) so the
/// assertion is a trivial `assert_eq!` against literal bytes — the
/// plan's "Done when" criterion.
const TRAFFIC_BRIEF_GOLDEN: &str = "\
---
name: traffic-modernisation
inputs: []
---

# Traffic modernisation

<!-- One-paragraph framing of what this change is trying to
     achieve. Plans reference this brief via `change.md`. -->
";

fn brief_path(project: &Project) -> PathBuf {
    project.root().join("change.md")
}

fn write_brief(project: &Project, body: &str) {
    fs::write(brief_path(project), body).expect("write change.md");
}

fn today_yyyymmdd() -> String {
    jiff::Timestamp::now().strftime("%Y%m%d").to_string()
}

#[test]
fn create_scaffolds_canonical_file() {
    let project = Project::init();
    assert!(!brief_path(&project).exists(), "bare project must not have change.md");

    specify()
        .current_dir(project.root())
        .args(["change", "create", "traffic-modernisation"])
        .assert()
        .success();

    let on_disk = fs::read_to_string(brief_path(&project)).expect("read change.md");
    assert_eq!(on_disk, TRAFFIC_BRIEF_GOLDEN);
}

#[test]
fn create_json_response() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "my-change"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert!(actual["action"].is_null(), "BriefCreateBody no longer carries an `action` field");
    assert!(actual["error"].is_null(), "success envelope must omit error: {actual}");
    assert_eq!(actual["name"], "my-change");
    assert!(
        actual["path"].as_str().expect("path string").ends_with("/change.md"),
        "path should point at the brief, got: {}",
        actual["path"]
    );
}

#[test]
fn create_refuses_when_file_exists() {
    let project = Project::init();
    write_brief(&project, "---\nname: pre-existing\n---\n\nhands off\n");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "pre-existing"])
        .assert()
        .failure();
    // Canonical ErrorBody envelope: kebab discriminant in `error`,
    // formatted message in `message`.
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "already-exists");
    let msg = actual["message"].as_str().expect("message");
    assert!(
        msg.starts_with("already-exists: change brief already exists at "),
        "message must start with the kebab discriminant + path; got: {msg}"
    );

    let on_disk = fs::read_to_string(brief_path(&project)).expect("read");
    assert_eq!(on_disk, "---\nname: pre-existing\n---\n\nhands off\n");
}

#[test]
fn create_rejects_non_kebab_name() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "NotKebab"])
        .assert()
        .failure();
    // Failure envelopes are written to stderr.
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "change-brief-name-not-kebab");
    let msg = actual["message"].as_str().expect("message");
    assert!(msg.contains("kebab-case"), "msg should mention kebab-case: {msg}");
    assert!(msg.contains("NotKebab"), "msg should mention the bad name: {msg}");
    assert!(!brief_path(&project).exists(), "no file should have been created");
}

/// RFC-3a C14 archive-sweep hook: the operator brief travels with the
/// archive. Real C33 sweep adds `workspace.md` + `slices/`; this test
/// pins the brief half.
#[test]
fn archive_includes_change_md() {
    let project = Project::init();
    project.seed_plan(
        "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: done
",
    );
    write_brief(&project, TRAFFIC_BRIEF_GOLDEN);

    specify().current_dir(project.root()).args(["change", "plan", "archive"]).assert().success();

    assert!(!brief_path(&project).exists(), "change.md must leave the repo root");

    let archived_dir =
        project.root().join(".specify/archive/plans").join(format!("demo-{}", today_yyyymmdd()));
    let archived_brief = archived_dir.join("change.md");
    assert!(archived_brief.exists(), "archived change.md missing at {}", archived_brief.display());
    let contents = fs::read_to_string(&archived_brief).expect("read archived brief");
    assert_eq!(contents, TRAFFIC_BRIEF_GOLDEN, "archived bytes must match source bytes");
}
