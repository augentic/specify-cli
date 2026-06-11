//! Integration tests for `specify workspace *` (workspace orchestration contract).
//!
//! Covers `workspace sync`, `workspace push`, and the hidden
//! `workspace prepare` executor helper. Selector
//! preflight, slot enrichment, and branch-preparation diagnostics are
//! pinned to the wire shape skill authors rely on.

use std::fs;

use tempfile::tempdir;

mod common;
use common::{Project, init_workspace, omnia_schema_dir, parse_stdout, run_git, specify_cmd};

#[test]
fn workspace_help_lists_active_subcommands() {
    let assert = specify_cmd().args(["workspace", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["sync", "push"] {
        assert!(
            stdout.contains(verb),
            "expected `workspace --help` to mention `{verb}`, got:\n{stdout}",
        );
    }
}

#[test]
fn c01_sync_unknown_selector_preflight() {
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();
    let gitignore_before = fs::read_to_string(tmp.path().join(".gitignore")).ok();

    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "sync", "ghost"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-project-selector-unknown");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("unknown project"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace/ghost").exists(),
        "unknown selector must fail before materialising the requested slot"
    );
    assert!(
        !tmp.path().join(".specify/workspace/alpha").exists(),
        "unknown selector must fail before syncing any registry project"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join(".gitignore")).ok(),
        gitignore_before,
        "unknown selector must fail before sync mutates .gitignore again"
    );
}

#[test]
fn c01_sync_skips_unselected_slots() {
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");
    for name in ["billing", "orders", "inventory"] {
        fs::create_dir_all(tmp.path().join(name)).unwrap();
    }
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: billing\n\
         \x20\x20\x20\x20url: ./billing\n\
         \x20\x20\x20\x20adapter: omnia@v1\n\
         \x20\x20\x20\x20description: billing service\n\
         \x20\x20- name: orders\n\
         \x20\x20\x20\x20url: ./orders\n\
         \x20\x20\x20\x20adapter: omnia@v1\n\
         \x20\x20\x20\x20description: orders service\n\
         \x20\x20- name: inventory\n\
         \x20\x20\x20\x20url: ./inventory\n\
         \x20\x20\x20\x20adapter: omnia@v1\n\
         \x20\x20\x20\x20description: inventory service\n",
    )
    .unwrap();

    specify_cmd()
        .current_dir(tmp.path())
        .args(["workspace", "sync", "orders", "billing"])
        .assert()
        .success();
    assert!(tmp.path().join(".specify/workspace/billing").exists());
    assert!(tmp.path().join(".specify/workspace/orders").exists());
    assert!(
        !tmp.path().join(".specify/workspace/inventory").exists(),
        "selected sync must not materialise unselected slots"
    );
}

#[test]
fn c01_sync_journals_completed_event() {
    // workflow §Observability: one `workspace.sync.completed` per
    // successful sync, carrying the materialised project names.
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");
    for name in ["billing", "orders"] {
        fs::create_dir_all(tmp.path().join(name)).unwrap();
    }
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: billing\n\
         \x20\x20\x20\x20url: ./billing\n\
         \x20\x20\x20\x20adapter: omnia@v1\n\
         \x20\x20- name: orders\n\
         \x20\x20\x20\x20url: ./orders\n\
         \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();

    specify_cmd().current_dir(tmp.path()).args(["workspace", "sync"]).assert().success();

    let raw = fs::read_to_string(tmp.path().join(".specify/journal.jsonl"))
        .expect("sync must journal workspace.sync.completed");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one event per sync, got:\n{raw}");
    assert!(lines[0].contains(r#""event":"workspace.sync.completed""#), "got:\n{}", lines[0]);
    assert!(lines[0].contains(r#""projects":["billing","orders"]"#), "got:\n{}", lines[0]);
}

#[test]
fn sync_no_registry_no_journal() {
    let project = Project::init();
    specify_cmd().current_dir(project.root()).args(["workspace", "sync"]).assert().success();
    assert!(
        !project.root().join(".specify/journal.jsonl").exists(),
        "the registry-less no-op sync must not journal workspace.sync.completed"
    );
}

#[test]
fn push_journals_completed_event() {
    // workflow §Observability: one `workspace.push.completed` per
    // successful non-dry-run push (a `local-only` outcome is not a
    // failure); dry runs emit nothing.
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    // A local git worktree without an `origin` remote resolves to the
    // `local-only` push outcome — success without network.
    let alpha = tmp.path().join("alpha");
    fs::create_dir_all(&alpha).unwrap();
    run_git(&alpha, &["init", "-b", "main"]);
    fs::write(alpha.join("README.md"), "seed\n").unwrap();
    run_git(&alpha, &["add", "README.md"]);
    run_git(&alpha, &["commit", "--no-gpg-sign", "-m", "seed"]);
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: ./alpha\n\
         \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();

    specify_cmd()
        .current_dir(tmp.path())
        .args(["workspace", "push", "--dry-run"])
        .assert()
        .success();
    assert!(
        !tmp.path().join(".specify/journal.jsonl").exists(),
        "--dry-run must not journal workspace.push.completed"
    );

    specify_cmd().current_dir(tmp.path()).args(["workspace", "push"]).assert().success();

    let raw = fs::read_to_string(tmp.path().join(".specify/journal.jsonl"))
        .expect("push must journal workspace.push.completed");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one event per push, got:\n{raw}");
    assert!(lines[0].contains(r#""event":"workspace.push.completed""#), "got:\n{}", lines[0]);
    assert!(lines[0].contains(r#""plan-name":"demo-change""#), "got:\n{}", lines[0]);
    assert!(lines[0].contains(r#""branch":"specify/demo-change""#), "got:\n{}", lines[0]);
    assert!(lines[0].contains(r#""projects":["alpha"]"#), "got:\n{}", lines[0]);
}

#[test]
fn c01_push_unknown_selector_preflight() {
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();

    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "push", "ghost", "--dry-run"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-project-selector-unknown");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("unknown project"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace/ghost").exists(),
        "unknown selector must fail before materialising the requested slot"
    );
    assert!(
        !tmp.path().join(".specify/workspace/alpha").exists(),
        "unknown selector must fail before push touches registry project slots"
    );
}

#[test]
fn c04_prepare_returns_json() {
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");

    let alpha = tmp.path().join("alpha");
    fs::create_dir_all(&alpha).unwrap();
    run_git(&alpha, &["init", "-b", "main"]);
    fs::write(alpha.join("README.md"), "seed\n").unwrap();
    run_git(&alpha, &["add", "README.md"]);
    run_git(&alpha, &["commit", "--no-gpg-sign", "-m", "seed"]);
    let remote = tmp.path().join("alpha.git");
    run_git(tmp.path(), &["clone", "--bare", alpha.to_str().unwrap(), remote.to_str().unwrap()]);
    let remote_url = format!("file://{}", remote.display());
    run_git(&alpha, &["remote", "add", "origin", &remote_url]);

    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: ./alpha\n\
         \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();

    let help = specify_cmd().args(["workspace", "--help"]).assert().success();
    let help_stdout = String::from_utf8(help.get_output().stdout.clone()).expect("help utf8");
    assert!(
        !help_stdout.contains("prepare"),
        "executor helper must stay hidden from human workspace help, got:\n{help_stdout}"
    );

    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "prepare", "alpha", "--change", "demo-change"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");

    assert_eq!(value["prepared"], true);
    assert_eq!(value["project"], "alpha");
    assert_eq!(value["branch"], "specify/demo-change");
    assert_eq!(value["local-branch"], "created");
    assert_eq!(value["remote-branch"], "absent");
    assert_eq!(value["dirty"]["tracked-blocked"], serde_json::json!([]));
    assert!(
        value.get("diagnostics").is_none(),
        "PrepareBody no longer carries a diagnostics field, got: {value}"
    );
    assert_eq!(run_git(&alpha, &["branch", "--show-current"]).trim(), "specify/demo-change");
}

#[test]
fn c04_prepare_origin_head_diagnostic() {
    let tmp = tempdir().unwrap();
    init_workspace(&tmp, "platform-workspace");

    let remote = tmp.path().join("headless.git");
    run_git(tmp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    let remote_url = format!("file://{}", remote.display());

    let alpha = tmp.path().join("alpha");
    fs::create_dir_all(&alpha).unwrap();
    run_git(&alpha, &["init", "-b", "main"]);
    run_git(&alpha, &["remote", "add", "origin", &remote_url]);
    fs::write(alpha.join("README.md"), "seed\n").unwrap();
    run_git(&alpha, &["add", "README.md"]);
    run_git(&alpha, &["commit", "--no-gpg-sign", "-m", "seed"]);

    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: ./alpha\n\
         \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();

    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "prepare", "alpha", "--change", "demo-change"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");

    assert_eq!(value["error"], "branch-preparation-failed");
    assert_eq!(value["exit-code"], 1);
    let message = value["message"].as_str().expect("message string");
    assert!(message.contains("alpha"), "message names the project: {message}");
    assert!(
        message.contains("origin-head-unresolved"),
        "message surfaces the diagnostic key: {message}"
    );
    assert_eq!(run_git(&alpha, &["branch", "--show-current"]).trim(), "main");
}

// ---- planning-path workspace smoke — planning-path workspace smoke ----
#[test]
fn planning_sync_no_registry_exits_zero() {
    let project = Project::init();
    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "workspace", "sync"])
        .assert()
        .success();
    let v = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(v["synced"], false);
    assert!(v["message"].as_str().unwrap().contains("no registry"));
}

#[test]
fn planning_sync_two_symlink_peers() {
    let tmp = tempdir().expect("tempdir");
    let peer = tmp.path().join("peer-proj");
    fs::create_dir_all(peer.join(".specify")).expect("peer .specify");
    let root = tmp.path().join("root");
    fs::create_dir_all(&root).expect("root");
    specify_cmd()
        .current_dir(&root)
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "planning-ws"])
        .assert()
        .success();

    let reg = "\
version: 1
projects:
  - name: alpha
    url: .
    adapter: omnia@v1
    description: Root project
  - name: beta
    url: ../peer-proj
    adapter: omnia@v1
    description: Peer project
";
    fs::write(root.join("registry.yaml"), reg).expect("registry");

    specify_cmd().current_dir(&root).args(["workspace", "sync"]).assert().success();

    assert!(root.join(".specify/workspace/alpha").exists());
    assert!(root.join(".specify/workspace/beta").exists());

    assert!(root.join(".specify/workspace/alpha").exists());
    assert!(root.join(".specify/workspace/beta").exists());
}
