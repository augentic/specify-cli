//! Integration tests for `specify workspace *` (RFC-14).
//!
//! Covers `workspace sync`, `workspace status`, `workspace push`, and
//! the hidden `workspace prepare-branch` executor helper. Selector
//! preflight, slot enrichment, and branch-preparation diagnostics are
//! pinned to the wire shape skill authors rely on.

use std::fs;

use tempfile::tempdir;

mod common;
use common::{Project, init_hub, parse_stdout, repo_root, run_git, specify};

#[test]
fn workspace_help_lists_active_subcommands() {
    let assert = specify().args(["workspace", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["sync", "status", "push"] {
        assert!(
            stdout.contains(verb),
            "expected `workspace --help` to mention `{verb}`, got:\n{stdout}",
        );
    }
}

#[test]
fn rfc14_c01_workspace_sync_unknown_selector_fails_before_side_effects() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20capability: omnia@v1\n",
    )
    .unwrap();
    let gitignore_before = fs::read_to_string(tmp.path().join(".gitignore")).ok();

    let assert = specify()
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
        !tmp.path().join(".specify/workspace").exists(),
        "unknown selector must fail before workspace materialisation"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join(".gitignore")).ok(),
        gitignore_before,
        "unknown selector must fail before sync mutates .gitignore"
    );
}

#[test]
fn rfc14_c01_workspace_status_unknown_selector_fails_before_side_effects() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20capability: omnia@v1\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "status", "ghost"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-project-selector-unknown");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("unknown project"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace").exists(),
        "status selector preflight must not materialise workspace paths"
    );
}

#[test]
fn rfc14_c01_workspace_sync_and_status_select_projects_in_registry_order() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    for name in ["billing", "orders", "inventory"] {
        fs::create_dir_all(tmp.path().join(name)).unwrap();
    }
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: billing\n\
         \x20\x20\x20\x20url: ./billing\n\
         \x20\x20\x20\x20capability: omnia@v1\n\
         \x20\x20\x20\x20description: billing service\n\
         \x20\x20- name: orders\n\
         \x20\x20\x20\x20url: ./orders\n\
         \x20\x20\x20\x20capability: omnia@v1\n\
         \x20\x20\x20\x20description: orders service\n\
         \x20\x20- name: inventory\n\
         \x20\x20\x20\x20url: ./inventory\n\
         \x20\x20\x20\x20capability: omnia@v1\n\
         \x20\x20\x20\x20description: inventory service\n",
    )
    .unwrap();

    specify()
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

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "status", "orders", "billing"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let names: Vec<&str> = value["slots"]
        .as_array()
        .expect("slots array")
        .iter()
        .map(|slot| slot["name"].as_str().expect("slot name"))
        .collect();
    assert_eq!(
        names,
        ["billing", "orders"],
        "selectors must preserve registry order, not argument order"
    );
}

#[test]
fn rfc14_c03_workspace_status_json_reports_enriched_slot_fields() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::create_dir_all(tmp.path().join("billing/.specify/slices/alpha")).unwrap();
    fs::write(
        tmp.path().join("billing/.specify/project.yaml"),
        "name: billing\ncapability: omnia@v1\n",
    )
    .unwrap();
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: billing\n\
         \x20\x20\x20\x20url: ./billing\n\
         \x20\x20\x20\x20capability: omnia@v1\n\
         \x20\x20\x20\x20description: billing service\n\
         \x20\x20- name: remote\n\
         \x20\x20\x20\x20url: git@github.com:org/remote.git\n\
         \x20\x20\x20\x20capability: omnia@v1\n\
         \x20\x20\x20\x20description: remote service\n",
    )
    .unwrap();

    specify().current_dir(tmp.path()).args(["workspace", "sync", "billing"]).assert().success();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "status", "remote", "billing"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let slots = value["slots"].as_array().expect("slots array");
    assert_eq!(slots.len(), 2);

    let billing = &slots[0];
    assert_eq!(billing["name"], "billing");
    assert_eq!(billing["kind"], "symlink");
    assert!(billing["slot-path"].as_str().unwrap().ends_with(".specify/workspace/billing"));
    assert_eq!(billing["configured-target-kind"], "local");
    assert!(billing["configured-target"].as_str().unwrap().ends_with("/billing"));
    assert!(billing["actual-symlink-target"].as_str().unwrap().ends_with("/billing"));
    assert_eq!(billing["project-config-present"], true);
    assert_eq!(billing["active-slices"], serde_json::json!(["alpha"]));
    assert!(billing["actual-origin"].is_null());
    assert!(billing["current-branch"].is_null());
    assert!(billing["branch-matches-change"].is_null());

    let remote = &slots[1];
    assert_eq!(remote["name"], "remote");
    assert_eq!(remote["kind"], "missing");
    assert_eq!(remote["configured-target-kind"], "remote");
    assert_eq!(remote["configured-target"], "git@github.com:org/remote.git");
    assert_eq!(remote["project-config-present"], false);
    assert_eq!(remote["active-slices"], serde_json::json!([]));
}

#[test]
fn rfc14_c03_workspace_status_text_flags_mismatch_dirty_and_project_config() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    let slot_path = tmp.path().join(".specify/workspace/remote");
    let remote_url = "git@github.com:org/remote.git";
    fs::create_dir_all(slot_path.join(".specify")).unwrap();
    fs::write(slot_path.join(".specify/project.yaml"), "name: remote\ncapability: omnia@v1\n")
        .unwrap();
    fs::write(slot_path.join("README.md"), "# remote\n").unwrap();
    run_git(&slot_path, &["init"]);
    run_git(&slot_path, &["remote", "add", "origin", remote_url]);
    run_git(&slot_path, &["add", "."]);
    run_git(&slot_path, &["commit", "-m", "initial"]);
    run_git(&slot_path, &["checkout", "-b", "feature/work"]);
    fs::write(slot_path.join("dirty.txt"), "dirty\n").unwrap();
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: remote\n\
         \x20\x20\x20\x20url: git@github.com:org/remote.git\n\
         \x20\x20\x20\x20capability: omnia@v1\n\
         \x20\x20\x20\x20description: remote service\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["workspace", "status", "remote"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");

    assert!(stdout.contains("remote: kind=git-clone"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("configured-remote=git@github.com:org/remote.git"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("origin=git@github.com:org/remote.git"), "stdout:\n{stdout}");
    assert!(stdout.contains("branch=feature/work"), "stdout:\n{stdout}");
    assert!(stdout.contains("change-branch=mismatch"), "stdout:\n{stdout}");
    assert!(stdout.contains("dirty=yes"), "stdout:\n{stdout}");
    assert!(stdout.contains("project.yaml=present"), "stdout:\n{stdout}");
}

#[test]
fn rfc14_c01_workspace_push_unknown_selector_fails_before_side_effects() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20capability: omnia@v1\n",
    )
    .unwrap();

    let assert = specify()
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
        !tmp.path().join(".specify/workspace").exists(),
        "unknown selector must fail before workspace paths are touched"
    );
}

#[test]
fn rfc14_c04_workspace_prepare_branch_hidden_helper_returns_structured_json() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

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
         \x20\x20\x20\x20capability: omnia@v1\n",
    )
    .unwrap();

    let help = specify().args(["workspace", "--help"]).assert().success();
    let help_stdout = String::from_utf8(help.get_output().stdout.clone()).expect("help utf8");
    assert!(
        !help_stdout.contains("prepare-branch"),
        "executor helper must stay hidden from human workspace help, got:\n{help_stdout}"
    );

    let assert = specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "workspace",
            "prepare-branch",
            "alpha",
            "--change",
            "demo-change",
        ])
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
        "PrepareBranchBody no longer carries a diagnostics field, got: {value}"
    );
    assert_eq!(run_git(&alpha, &["branch", "--show-current"]).trim(), "specify/demo-change");
}

#[test]
fn rfc14_c04_workspace_prepare_branch_surfaces_origin_head_diagnostic_key() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

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
         \x20\x20\x20\x20capability: omnia@v1\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "workspace",
            "prepare-branch",
            "alpha",
            "--change",
            "demo-change",
        ])
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

// ---- RFC-3a C35 — planning-path workspace smoke ----
#[test]
fn rfc3a_c35_workspace_sync_absent_registry_exits_zero() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "workspace", "sync"])
        .assert()
        .success();
    let v = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(v["synced"], false);
    assert!(v["message"].as_str().unwrap().contains("no registry"));
}

#[test]
fn rfc3a_c35_workspace_sync_two_local_symlink_peers() {
    let tmp = tempdir().expect("tempdir");
    let peer = tmp.path().join("peer-proj");
    fs::create_dir_all(peer.join(".specify")).expect("peer .specify");
    let root = tmp.path().join("root");
    fs::create_dir_all(&root).expect("root");
    specify()
        .current_dir(&root)
        .args(["init"])
        .arg(repo_root().join("schemas").join("omnia"))
        .args(["--name", "rfc3a-ws"])
        .assert()
        .success();

    let reg = "\
version: 1
projects:
  - name: alpha
    url: .
    capability: omnia@v1
    description: Root project
  - name: beta
    url: ../peer-proj
    capability: omnia@v1
    description: Peer project
";
    fs::write(root.join("registry.yaml"), reg).expect("registry");

    specify().current_dir(&root).args(["workspace", "sync"]).assert().success();

    assert!(root.join(".specify/workspace/alpha").exists());
    assert!(root.join(".specify/workspace/beta").exists());

    let assert_st = specify()
        .current_dir(&root)
        .args(["--format", "json", "workspace", "status"])
        .assert()
        .success();
    let v = parse_stdout(&assert_st.get_output().stdout, &root);
    let slots = v["slots"].as_array().expect("slots array");
    assert_eq!(slots.len(), 2);
    let kinds: Vec<&str> = slots.iter().map(|s| s["kind"].as_str().expect("kind")).collect();
    assert!(kinds.contains(&"symlink"), "expected symlink slots, got {kinds:?}");
}
