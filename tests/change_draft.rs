//! Integration tests for `specify change create` — scaffolds both the
//! operator brief (`change.md`) and the plan (`plan.yaml`) at the repo
//! root in a single atomic write. Template byte stability is the key
//! contract: `create` must produce the same brief bytes every time so
//! operators can diff against the RFC-matching golden, and refuse
//! atomically (writing nothing) when either output already exists.

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
fn create_scaffolds_canonical_brief_and_plan() {
    let project = Project::init();
    assert!(!brief_path(&project).exists(), "bare project must not have change.md");
    assert!(!project.plan_path().exists(), "bare project must not have plan.yaml");

    specify()
        .current_dir(project.root())
        .args(["change", "create", "traffic-modernisation"])
        .assert()
        .success();

    let on_disk = fs::read_to_string(brief_path(&project)).expect("read change.md");
    assert_eq!(on_disk, TRAFFIC_BRIEF_GOLDEN);

    let plan_yaml = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(
        plan_yaml.contains("name: traffic-modernisation"),
        "plan.yaml must carry the change name, got:\n{plan_yaml}"
    );
}

#[test]
fn create_records_sources_in_plan() {
    let project = Project::init();

    specify()
        .current_dir(project.root())
        .args([
            "change",
            "create",
            "big",
            "--source",
            "monolith=/tmp/legacy",
            "--source",
            "orders=git@github.com:org/orders.git",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(saved.contains("name: big"), "plan missing name:\n{saved}");
    assert!(saved.contains("monolith: /tmp/legacy"), "plan missing monolith source:\n{saved}");
    assert!(
        saved.contains("orders: git@github.com:org/orders.git"),
        "plan missing orders source:\n{saved}"
    );
    let brief = fs::read_to_string(brief_path(&project)).expect("read change.md");
    assert!(brief.contains("name: big"), "brief frontmatter missing name:\n{brief}");
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
    assert!(actual["action"].is_null(), "CreateBody does not carry an `action` field");
    assert!(actual["error"].is_null(), "success envelope must omit error: {actual}");
    assert_eq!(actual["name"], "my-change");
    assert!(
        actual["brief"].as_str().expect("brief string").ends_with("/change.md"),
        "brief should point at change.md, got: {}",
        actual["brief"]
    );
    assert!(
        actual["plan"].as_str().expect("plan string").ends_with("/plan.yaml"),
        "plan should point at plan.yaml, got: {}",
        actual["plan"]
    );
}

#[test]
fn create_refuses_atomically_when_brief_exists() {
    let project = Project::init();
    write_brief(&project, "---\nname: pre-existing\n---\n\nhands off\n");
    assert!(!project.plan_path().exists(), "fixture must start without a plan.yaml");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "pre-existing"])
        .assert()
        .failure();
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "already-exists");
    let msg = actual["message"].as_str().expect("message");
    assert!(msg.contains("change brief at"), "message must name the brief collision; got: {msg}");

    let on_disk = fs::read_to_string(brief_path(&project)).expect("read");
    assert_eq!(on_disk, "---\nname: pre-existing\n---\n\nhands off\n");
    assert!(!project.plan_path().exists(), "atomic refusal must not have written plan.yaml");
}

#[test]
fn create_refuses_atomically_when_plan_exists() {
    let project = Project::init();
    project.seed_plan("name: pre-existing\nslices: []\n");
    assert!(!brief_path(&project).exists(), "fixture must start without a change.md");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "pre-existing"])
        .assert()
        .failure();
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "already-exists");
    let msg = actual["message"].as_str().expect("message");
    assert!(msg.contains("plan at"), "message must name the plan collision; got: {msg}");

    assert!(!brief_path(&project).exists(), "atomic refusal must not have written change.md");
}

#[test]
fn create_rejects_non_kebab_name() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "NotKebab"])
        .assert()
        .failure();
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "change-name-not-kebab");
    let msg = actual["message"].as_str().expect("message");
    assert!(msg.contains("kebab-case"), "msg should mention kebab-case: {msg}");
    assert!(msg.contains("NotKebab"), "msg should mention the bad name: {msg}");
    assert!(!brief_path(&project).exists(), "no change.md should have been created");
    assert!(!project.plan_path().exists(), "no plan.yaml should have been created");
}

#[test]
fn create_rejects_duplicate_source_key() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format", "json", "change", "create", "x", "--source", "a=/p1", "--source", "a=/p2",
        ])
        .assert()
        .failure();
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "plan-source-duplicate-key");
    assert!(!brief_path(&project).exists(), "no change.md on duplicate source key");
    assert!(!project.plan_path().exists(), "no plan.yaml on duplicate source key");
}

#[test]
fn create_rejects_malformed_source() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["change", "create", "x", "--source", "badkey"])
        .assert()
        .failure();
    assert_eq!(
        assert.get_output().status.code(),
        Some(2),
        "clap parse errors must surface as exit code 2"
    );
    assert!(!brief_path(&project).exists(), "no change.md on malformed --source");
    assert!(!project.plan_path().exists(), "no plan.yaml on malformed --source");
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

    specify().current_dir(project.root()).args(["plan", "archive"]).assert().success();

    assert!(!brief_path(&project).exists(), "change.md must leave the repo root");

    let archived_dir =
        project.root().join(".specify/archive/plans").join(format!("demo-{}", today_yyyymmdd()));
    let archived_brief = archived_dir.join("change.md");
    assert!(archived_brief.exists(), "archived change.md missing at {}", archived_brief.display());
    let contents = fs::read_to_string(&archived_brief).expect("read archived brief");
    assert_eq!(contents, TRAFFIC_BRIEF_GOLDEN, "archived bytes must match source bytes");
}
