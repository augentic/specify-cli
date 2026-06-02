//! Integration tests for `specify_workflow::registry::workspace` and
//! `specify_workflow::registry::forge`.
//!
//! Pins the public surface lifted from the binary by the workspace split 2.2:
//! `github_slug`, `sync_projects` (registry-absent short-circuit at the
//! caller, `.gitignore` upkeep), `workspace_status` (returns `None`
//! when no registry), `is_specify_branch`, and `branches_match`.
//! Per-classifier coverage continues to live in the in-module
//! `#[cfg(test)]` blocks; this file exercises the integration boundary
//! an external consumer (the binary, plus anyone replacing it via
//! `--lib`) would touch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use specify_workflow::registry::branch::{
    LocalAction, RemoteAction, Request as BranchRequest, prepare,
};
use specify_workflow::registry::forge::{branches_match, is_specify_branch};
use specify_workflow::registry::workspace::{
    PushOutcome, SlotKind, SlotProblemReason, SlotStatus, github_slug, push_projects, slot_problem,
    status as workspace_status, status_projects as workspace_status_projects,
    sync_projects as workspace_sync_projects,
};
use specify_workflow::registry::{Registry, RegistryProject};
use tempfile::TempDir;

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_dir(target, link).expect("symlink");
}

const GIT_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Specify Test"),
    ("GIT_AUTHOR_EMAIL", "specify-test@example.com"),
    ("GIT_COMMITTER_NAME", "Specify Test"),
    ("GIT_COMMITTER_EMAIL", "specify-test@example.com"),
];

fn run_git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .envs(GIT_ENV)
        .output()
        .unwrap_or_else(|err| panic!("git {} failed to start: {err}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout utf8")
}

fn registry_with_projects(names: &[&str]) -> Registry {
    Registry {
        version: 1,
        projects: names
            .iter()
            .map(|name| RegistryProject {
                name: (*name).to_string(),
                url: format!("./{name}"),
                adapter: Some("omnia@v1".to_string()),
                description: Some(format!("{name} service")),
                contracts: None,
            })
            .collect(),
    }
}

fn branch_request(change_name: &str) -> BranchRequest {
    BranchRequest {
        change_name: change_name.to_string(),
        source_paths: Vec::new(),
        output_paths: Vec::new(),
    }
}

fn remote_project(url: String) -> RegistryProject {
    RegistryProject {
        name: "alpha".to_string(),
        url,
        adapter: Some("omnia@v1".to_string()),
        description: Some("alpha service".to_string()),
        contracts: None,
    }
}

fn seed_bare_remote(tmp: &TempDir) -> (PathBuf, String) {
    let source = tmp.path().join("source");
    fs::create_dir_all(&source).unwrap();
    run_git(&source, &["init", "-b", "main"]);
    fs::write(source.join("README.md"), "seed\n").unwrap();
    run_git(&source, &["add", "README.md"]);
    run_git(&source, &["commit", "--no-gpg-sign", "-m", "seed"]);

    let remote = tmp.path().join("alpha.git");
    run_git(tmp.path(), &["clone", "--bare", source.to_str().unwrap(), remote.to_str().unwrap()]);
    (remote.clone(), format!("file://{}", remote.display()))
}

fn clone_workspace_slot(project_dir: &Path, remote_url: &str) -> PathBuf {
    let slot = project_dir.join(".specify/workspace/alpha");
    fs::create_dir_all(slot.parent().unwrap()).unwrap();
    run_git(project_dir, &["clone", remote_url, slot.to_str().unwrap()]);
    slot
}

fn git_output(root: &Path, args: &[&str]) -> String {
    run_git(root, args).trim().to_string()
}

fn current_branch(root: &Path) -> String {
    git_output(root, &["branch", "--show-current"])
}

fn prepare_change_branch(slot: &Path, change_name: &str) -> String {
    let branch = format!("specify/{change_name}");
    run_git(slot, &["checkout", "-b", &branch]);
    fs::write(slot.join("change.txt"), format!("{change_name}\n")).unwrap();
    run_git(slot, &["add", "change.txt"]);
    run_git(slot, &["commit", "--no-gpg-sign", "-m", "change work"]);
    git_output(slot, &["rev-parse", "HEAD"])
}

fn remote_branch_head(remote_url: &str, branch: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["ls-remote", "--heads", remote_url, &format!("refs/heads/{branch}")])
        .output()
        .expect("spawn git ls-remote");
    assert!(
        output.status.success(),
        "git ls-remote failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.split_whitespace().next())
        .map(ToString::to_string)
}

// ---------- github_slug --------------------------------------

#[test]
fn github_slug_handles_each_supported_form() {
    assert_eq!(github_slug("git@github.com:org/mobile.git"), Some("org/mobile".to_string()));
    assert_eq!(github_slug("git@github.com:org/mobile"), Some("org/mobile".to_string()));
    assert_eq!(github_slug("https://github.com/org/mobile.git"), Some("org/mobile".to_string()));
    assert_eq!(github_slug("https://github.com/org/mobile"), Some("org/mobile".to_string()));
    assert_eq!(github_slug("ssh://git@github.com/org/mobile.git"), Some("org/mobile".to_string()));
    assert_eq!(github_slug("git@gitlab.com:org/repo.git"), None);
}

// ---------- sync_projects ---------------------------------------

#[test]
fn workspace_sync_no_registry_is_noop() {
    let tmp = TempDir::new().unwrap();
    assert!(
        Registry::load(tmp.path()).expect("registry load must not error").is_none(),
        "absent registry must yield None so callers can short-circuit",
    );
    assert!(!tmp.path().join(".gitignore").exists());
    assert!(!tmp.path().join(".specify/workspace").exists());
}

#[test]
fn sync_symlink_creates_workspace_and_gitignore() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();

    let peer_dir = project_dir.join("peer");
    fs::create_dir_all(&peer_dir).unwrap();

    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    adapter: omnia@v1
",
    )
    .unwrap();

    let registry = Registry::load(project_dir).unwrap().expect("registry present");
    workspace_sync_projects(project_dir, &registry.select(&[]).unwrap()).expect("sync ok");

    let slot = project_dir.join(".specify/workspace/peer");
    assert!(slot.exists(), "symlink slot must materialise");
    let meta = fs::symlink_metadata(&slot).unwrap();
    assert!(meta.file_type().is_symlink(), "symlink expected, got {meta:?}");
    let target = fs::canonicalize(&slot).unwrap();
    assert_eq!(target, fs::canonicalize(&peer_dir).unwrap());

    let gitignore = fs::read_to_string(project_dir.join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l.trim() == ".specify/workspace/"));
    assert!(gitignore.lines().any(|l| l.trim() == ".specify/.cache/"));
}

#[test]
fn c00_sync_materialises_all_projects() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("alpha")).unwrap();
    fs::create_dir_all(project_dir.join("beta")).unwrap();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: alpha
    url: ./alpha
    adapter: omnia@v1
    description: alpha service
  - name: beta
    url: ./beta
    adapter: omnia@v1
    description: beta service
",
    )
    .unwrap();

    let registry = Registry::load(project_dir).unwrap().expect("registry present");
    workspace_sync_projects(project_dir, &registry.select(&[]).unwrap()).expect("sync ok");

    let workspace = project_dir.join(".specify/workspace");
    for name in ["alpha", "beta"] {
        let slot = workspace.join(name);
        let meta = fs::symlink_metadata(&slot).unwrap();
        assert!(meta.file_type().is_symlink(), "{name} slot must materialise as a symlink");
    }

    let slots = workspace_status(project_dir).expect("ok").expect("registry present");
    let names: Vec<&str> = slots.iter().map(|slot| slot.name.as_str()).collect();
    assert_eq!(
        names,
        ["alpha", "beta"],
        "pre-workspace orchestration contract sync/status without selectors cover every registry project"
    );
}

#[test]
fn c01_selector_preserves_order() {
    let registry = registry_with_projects(&["billing", "orders", "inventory"]);
    let selected =
        registry.select(&["orders".to_string(), "billing".to_string()]).expect("selectors resolve");
    let names: Vec<&str> = selected.iter().map(|project| project.name.as_str()).collect();
    assert_eq!(names, ["billing", "orders"]);
}

#[test]
fn c01_selector_resolver_rejects_unknown_project() {
    let registry = registry_with_projects(&["billing", "orders"]);
    let err = registry.select(&["ghost".to_string()]).expect_err("unknown selector must fail");
    let msg = err.to_string();
    assert!(msg.contains("unknown project selector"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(msg.contains("billing"), "msg: {msg}");
    assert!(msg.contains("orders"), "msg: {msg}");
}

#[test]
fn c01_sync_selected_slots_only() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    for name in ["billing", "orders", "inventory"] {
        fs::create_dir_all(project_dir.join(name)).unwrap();
    }
    let registry = registry_with_projects(&["billing", "orders", "inventory"]);
    let selected =
        registry.select(&["orders".to_string(), "billing".to_string()]).expect("selectors resolve");

    workspace_sync_projects(project_dir, &selected).expect("sync selected");

    assert!(project_dir.join(".specify/workspace/billing").exists());
    assert!(project_dir.join(".specify/workspace/orders").exists());
    assert!(
        !project_dir.join(".specify/workspace/inventory").exists(),
        "selected sync must not materialise unselected slots"
    );
}

#[test]
fn c01_status_selected_in_registry_order() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let registry = registry_with_projects(&["billing", "orders", "inventory"]);
    let selected =
        registry.select(&["orders".to_string(), "billing".to_string()]).expect("selectors resolve");

    let slots = workspace_status_projects(project_dir, &selected);

    let names: Vec<&str> = slots.iter().map(|slot| slot.name.as_str()).collect();
    assert_eq!(names, ["billing", "orders"]);
    assert!(slots.iter().all(|slot| slot.kind == SlotKind::Missing));
}

#[test]
fn c02_sync_recreates_deleted_slot() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    for name in ["billing", "orders"] {
        fs::create_dir_all(project_dir.join(name)).unwrap();
    }
    let registry = registry_with_projects(&["billing", "orders"]);
    let selected = registry.select(&["billing".to_string()]).expect("selectors resolve");

    let workspace = project_dir.join(".specify/workspace");
    fs::create_dir_all(workspace.join("orders")).unwrap();
    fs::write(workspace.join("orders").join("sentinel.txt"), "hands off").unwrap();

    workspace_sync_projects(project_dir, &selected).expect("sync billing");
    let billing_slot = workspace.join("billing");
    assert!(fs::symlink_metadata(&billing_slot).unwrap().file_type().is_symlink());

    fs::remove_file(&billing_slot).unwrap();
    workspace_sync_projects(project_dir, &selected).expect("resync billing");

    assert!(fs::symlink_metadata(&billing_slot).unwrap().file_type().is_symlink());
    assert_eq!(
        fs::read_to_string(workspace.join("orders").join("sentinel.txt")).unwrap(),
        "hands off",
        "selected sync must not touch unselected slot paths"
    );
}

#[test]
fn c02_local_slot_refuses_existing_non_symlink() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::create_dir_all(project_dir.join(".specify/workspace/peer")).unwrap();
    fs::write(project_dir.join(".specify/workspace/peer/sentinel.txt"), "keep").unwrap();

    let registry = registry_with_projects(&["peer"]);
    let selected = registry.select(&["peer".to_string()]).unwrap();
    let err = workspace_sync_projects(project_dir, &selected)
        .expect_err("mismatched local slot should fail");
    let msg = err.to_string();

    assert!(msg.contains("not a symlink"), "msg: {msg}");
    assert_eq!(
        fs::read_to_string(project_dir.join(".specify/workspace/peer/sentinel.txt")).unwrap(),
        "keep",
        "mismatched slot must not be overwritten"
    );
}

#[test]
fn c02_local_slot_wrong_symlink() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let peer = project_dir.join("peer");
    let other = project_dir.join("other");
    fs::create_dir_all(&peer).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::create_dir_all(project_dir.join(".specify/workspace")).unwrap();
    symlink_dir(&other, &project_dir.join(".specify/workspace/peer"));

    let registry = registry_with_projects(&["peer"]);
    let selected = registry.select(&["peer".to_string()]).unwrap();
    let err = workspace_sync_projects(project_dir, &selected)
        .expect_err("wrong symlink target should fail");
    let msg = err.to_string();

    assert!(msg.contains("symlink to"), "msg: {msg}");
    assert!(msg.contains(&fs::canonicalize(peer).unwrap().display().to_string()), "msg: {msg}");
    assert_eq!(
        fs::canonicalize(project_dir.join(".specify/workspace/peer")).unwrap(),
        fs::canonicalize(other).unwrap(),
        "wrong symlink target must be preserved for operator inspection"
    );
}

#[test]
fn c02_remote_slot_refuses_existing_symlink() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let target = project_dir.join("not-remote");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(project_dir.join(".specify/workspace")).unwrap();
    symlink_dir(&target, &project_dir.join(".specify/workspace/remote"));

    let project = RegistryProject {
        name: "remote".to_string(),
        url: "https://example.invalid/org/remote.git".to_string(),
        adapter: Some("https://example.invalid/adapter".to_string()),
        description: Some("remote service".to_string()),
        contracts: None,
    };
    let err = workspace_sync_projects(project_dir, &[&project])
        .expect_err("remote-backed symlink slot should fail");
    let msg = err.to_string();

    assert!(msg.contains("is a symlink"), "msg: {msg}");
    assert!(
        fs::symlink_metadata(project_dir.join(".specify/workspace/remote"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn c10_slot_problem_wrong_symlink() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let peer = project_dir.join("peer");
    let other = project_dir.join("other");
    fs::create_dir_all(&peer).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::create_dir_all(project_dir.join(".specify/workspace")).unwrap();
    symlink_dir(&other, &project_dir.join(".specify/workspace/peer"));

    let registry = registry_with_projects(&["peer"]);
    let project = &registry.projects[0];
    let problem = slot_problem(project_dir, project).expect("wrong symlink problem");
    assert_eq!(problem.reason, SlotProblemReason::SymlinkTargetMismatch);
    assert_eq!(problem.observed_kind, Some(SlotKind::Symlink));

    let selected = registry.select(&["peer".to_string()]).unwrap();
    let err = workspace_sync_projects(project_dir, &selected)
        .expect_err("sync should refuse same wrong symlink");
    let msg = err.to_string();
    assert!(msg.contains(problem.message()), "msg: {msg}\nproblem: {}", problem.message());
}

#[test]
fn c10_slot_problem_wrong_origin() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let slot = project_dir.join(".specify/workspace/remote");
    fs::create_dir_all(&slot).unwrap();
    run_git(&slot, &["init"]);
    run_git(&slot, &["remote", "add", "origin", "https://example.invalid/old.git"]);

    let project = RegistryProject {
        name: "remote".to_string(),
        url: "https://example.invalid/new.git".to_string(),
        adapter: Some("https://example.invalid/adapter".to_string()),
        description: Some("remote service".to_string()),
        contracts: None,
    };
    let problem = slot_problem(project_dir, &project).expect("wrong origin problem");
    assert_eq!(problem.reason, SlotProblemReason::RemoteOriginMismatch);
    assert_eq!(problem.observed_url.as_deref(), Some("https://example.invalid/old.git"));

    let err = workspace_sync_projects(project_dir, &[&project])
        .expect_err("sync should refuse same wrong origin");
    let msg = err.to_string();
    assert!(msg.contains(problem.message()), "msg: {msg}\nproblem: {}", problem.message());
}

#[test]
fn c02_sync_refuses_escaping_name() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    let project = RegistryProject {
        name: "../escape".to_string(),
        url: "./peer".to_string(),
        adapter: Some("omnia@v1".to_string()),
        description: Some("bad selector".to_string()),
        contracts: None,
    };

    let err = workspace_sync_projects(project_dir, &[&project])
        .expect_err("traversal-like project name should fail");
    let msg = err.to_string();

    assert!(msg.contains("single path component"), "msg: {msg}");
    assert!(
        !project_dir.join(".specify/escape").exists(),
        "sync must not materialise a path outside .specify/workspace/<project>/"
    );
}

#[test]
fn c02_sync_refuses_symlinked_workspace_base() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::create_dir_all(project_dir.join(".specify")).unwrap();
    let outside = project_dir.join("outside-workspace");
    fs::create_dir_all(&outside).unwrap();
    symlink_dir(&outside, &project_dir.join(".specify/workspace"));

    let registry = registry_with_projects(&["peer"]);
    let selected = registry.select(&["peer".to_string()]).unwrap();
    let err = workspace_sync_projects(project_dir, &selected)
        .expect_err("workspace base symlink should fail");
    let msg = err.to_string();

    assert!(msg.contains(".specify/workspace/ is a symlink"), "msg: {msg}");
    assert!(
        !outside.join("peer").exists(),
        "sync must not materialise slots through a symlinked workspace base"
    );
}

#[test]
fn c02_sync_preserves_gitignore_once() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::write(project_dir.join(".gitignore"), "target/\n.specify/workspace/\n").unwrap();
    let registry = registry_with_projects(&["peer"]);
    let selected = registry.select(&[]).unwrap();

    workspace_sync_projects(project_dir, &selected).expect("sync ok");
    workspace_sync_projects(project_dir, &selected).expect("sync remains idempotent");

    let gitignore = fs::read_to_string(project_dir.join(".gitignore")).unwrap();
    assert_eq!(gitignore.lines().filter(|line| line.trim() == ".specify/workspace/").count(), 1);
    assert_eq!(gitignore.lines().filter(|line| line.trim() == ".specify/.cache/").count(), 1);
}

// ---------- branch preparation (workspace orchestration contract C04) ----------------------------

#[test]
fn c04_prepare_creates_change_branch() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    let project = remote_project(remote_url);
    let origin_head = git_output(&slot, &["rev-parse", "origin/HEAD"]);

    let prepared = prepare(&project_dir, &project, &branch_request("demo-change"))
        .expect("branch preparation succeeds");

    assert_eq!(prepared.branch, "specify/demo-change");
    assert_eq!(prepared.local_branch, LocalAction::Created);
    assert_eq!(prepared.remote_branch, RemoteAction::Absent);
    assert_eq!(current_branch(&slot), "specify/demo-change");
    assert_eq!(git_output(&slot, &["rev-parse", "HEAD"]), origin_head);
    assert_eq!(prepared.base_ref, "refs/remotes/origin/main");
}

#[test]
fn c04_prepare_reuses_resume_dirty_ok() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    let project = remote_project(remote_url);
    prepare(&project_dir, &project, &branch_request("demo-change")).expect("initial prepare");

    let tracked = slot.join(".specify/slices/demo-change/notes.md");
    fs::create_dir_all(tracked.parent().unwrap()).unwrap();
    fs::write(&tracked, "first\n").unwrap();
    run_git(&slot, &["add", ".specify/slices/demo-change/notes.md"]);
    run_git(&slot, &["commit", "--no-gpg-sign", "-m", "slice progress"]);
    fs::write(&tracked, "first\nsecond\n").unwrap();
    fs::write(slot.join("scratch.tmp"), "untracked\n").unwrap();

    let prepared = prepare(&project_dir, &project, &branch_request("demo-change"))
        .expect("resume prepare accepts active slice dirtiness");

    assert_eq!(prepared.local_branch, LocalAction::Reused);
    assert_eq!(current_branch(&slot), "specify/demo-change");
    assert_eq!(
        prepared.dirty.tracked_allowed,
        vec![".specify/slices/demo-change/notes.md".to_string()]
    );
    assert_eq!(prepared.dirty.tracked_blocked, Vec::<String>::new());
    assert_eq!(prepared.dirty.untracked, vec!["scratch.tmp".to_string()]);
}

#[test]
fn c04_prepare_ff_remote_ahead() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    let project = remote_project(remote_url.clone());
    prepare(&project_dir, &project, &branch_request("demo-change")).expect("initial prepare");

    let peer = tmp.path().join("peer");
    run_git(tmp.path(), &["clone", &remote_url, peer.to_str().unwrap()]);
    run_git(&peer, &["checkout", "-b", "specify/demo-change"]);
    fs::write(peer.join("README.md"), "seed\nremote\n").unwrap();
    run_git(&peer, &["add", "README.md"]);
    run_git(&peer, &["commit", "--no-gpg-sign", "-m", "remote progress"]);
    run_git(&peer, &["push", "origin", "specify/demo-change"]);
    let remote_tip = git_output(&peer, &["rev-parse", "HEAD"]);

    let prepared = prepare(&project_dir, &project, &branch_request("demo-change"))
        .expect("remote ahead fast-forwards");

    assert_eq!(prepared.local_branch, LocalAction::Reused);
    assert_eq!(prepared.remote_branch, RemoteAction::FastForwarded);
    assert_eq!(git_output(&slot, &["rev-parse", "HEAD"]), remote_tip);
}

#[test]
fn c04_prepare_blocks_unrelated_dirty() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    let project = remote_project(remote_url);
    fs::write(slot.join("README.md"), "unrelated\n").unwrap();

    let err = prepare(&project_dir, &project, &branch_request("demo-change"))
        .expect_err("unrelated tracked work must block");

    assert_eq!(err.key, "dirty-unrelated-tracked");
    assert_eq!(err.paths, vec!["README.md".to_string()]);
    assert_eq!(current_branch(&slot), "main", "branch must not be changed on refusal");
}

#[test]
fn c04_prepare_branch_reports_missing_origin() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let slot = project_dir.join(".specify/workspace/alpha");
    fs::create_dir_all(&slot).unwrap();
    run_git(&slot, &["init", "-b", "main"]);
    fs::write(slot.join("README.md"), "seed\n").unwrap();
    run_git(&slot, &["add", "README.md"]);
    run_git(&slot, &["commit", "--no-gpg-sign", "-m", "seed"]);
    let project = remote_project("https://example.invalid/org/alpha.git".to_string());

    let err = prepare(&project_dir, &project, &branch_request("demo-change"))
        .expect_err("missing origin must fail");

    assert_eq!(err.key, "missing-origin");
    assert_eq!(current_branch(&slot), "main");
}

#[test]
fn c04_prepare_unresolved_origin_head() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("headless.git");
    run_git(tmp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    let remote_url = format!("file://{}", remote.display());

    let slot = project_dir.join(".specify/workspace/alpha");
    fs::create_dir_all(&slot).unwrap();
    run_git(&slot, &["init", "-b", "main"]);
    run_git(&slot, &["remote", "add", "origin", &remote_url]);
    fs::write(slot.join("README.md"), "seed\n").unwrap();
    run_git(&slot, &["add", "README.md"]);
    run_git(&slot, &["commit", "--no-gpg-sign", "-m", "seed"]);
    let project = remote_project(remote_url);

    let err = prepare(&project_dir, &project, &branch_request("demo-change"))
        .expect_err("unresolved origin HEAD must fail");

    assert_eq!(err.key, "origin-head-unresolved");
    assert_eq!(current_branch(&slot), "main");
    assert!(
        !git_output(&slot, &["branch", "--list", "specify/demo-change"])
            .contains("specify/demo-change"),
        "must not create a guessed branch when origin/HEAD is unresolved"
    );
}

// ---------- workspace_status -----------------------------------------

#[test]
fn status_none_without_registry() {
    let tmp = TempDir::new().unwrap();
    let result = workspace_status(tmp.path()).expect("ok");
    assert!(result.is_none(), "absent registry must yield None");
}

#[test]
fn status_missing_unrealised_slot() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: alpha
    url: git@github.com:org/alpha.git
    adapter: omnia@v1
    description: alpha
  - name: beta
    url: git@github.com:org/beta.git
    adapter: omnia@v1
    description: beta
",
    )
    .unwrap();

    let slots = workspace_status(project_dir).expect("ok").expect("registry present");
    assert_eq!(slots.len(), 2);
    assert!(
        slots.iter().all(|s| matches!(
            s,
            SlotStatus {
                kind: SlotKind::Missing,
                head_sha: None,
                dirty: None,
                ..
            }
        )),
        "unmaterialised slots must classify as Missing, got: {slots:?}",
    );
    assert_eq!(slots[0].name, "alpha");
    assert_eq!(slots[1].name, "beta");
}

#[test]
fn status_symlink_kind_after_sync() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    adapter: omnia@v1
",
    )
    .unwrap();

    let registry = Registry::load(project_dir).unwrap().expect("registry present");
    workspace_sync_projects(project_dir, &registry.select(&[]).unwrap()).unwrap();
    let slots = workspace_status(project_dir).unwrap().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].name, "peer");
    assert_eq!(slots[0].kind, SlotKind::Symlink);
    // No git work tree behind the symlink target — head/dirty stay None.
    assert!(slots[0].head_sha.is_none());
}

#[test]
fn c03_status_enriches_symlink_project_state() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let peer = project_dir.join("peer");
    fs::create_dir_all(peer.join(".specify/slices/zeta")).unwrap();
    fs::create_dir_all(peer.join(".specify/slices/alpha")).unwrap();
    fs::write(peer.join(".specify/project.yaml"), "name: peer\nadapter: omnia@v1\n").unwrap();
    fs::write(peer.join("README.md"), "# peer\n").unwrap();
    run_git(&peer, &["init"]);
    run_git(&peer, &["add", "."]);
    run_git(&peer, &["commit", "-m", "initial"]);
    run_git(&peer, &["checkout", "-b", "specify/demo-change"]);
    fs::write(project_dir.join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    adapter: omnia@v1
",
    )
    .unwrap();

    let registry = Registry::load(project_dir).unwrap().expect("registry present");
    workspace_sync_projects(project_dir, &registry.select(&[]).unwrap()).unwrap();
    let slots = workspace_status(project_dir).unwrap().unwrap();
    let slot = &slots[0];

    assert_eq!(slot.kind, SlotKind::Symlink);
    assert_eq!(slot.slot_path, project_dir.join(".specify/workspace/peer"));
    assert_eq!(
        slot.configured_target_kind,
        specify_workflow::registry::workspace::ConfiguredTargetKind::Local
    );
    assert_eq!(slot.configured_target, fs::canonicalize(&peer).unwrap().display().to_string());
    assert_eq!(slot.actual_symlink_target, Some(fs::canonicalize(&peer).unwrap()));
    assert_eq!(slot.current_branch.as_deref(), Some("specify/demo-change"));
    assert!(slot.head_sha.as_ref().is_some_and(|sha| sha.len() == 40));
    assert_eq!(slot.dirty, Some(false));
    assert_eq!(slot.branch_matches_change, Some(true));
    assert!(slot.project_config_present);
    assert_eq!(slot.active_slices, ["alpha", "zeta"]);
}

#[test]
fn c03_status_git_clone_mismatch() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let slot_path = project_dir.join(".specify/workspace/remote");
    let remote_url = "https://example.invalid/org/remote.git";
    fs::create_dir_all(slot_path.join(".specify/slices/draft")).unwrap();
    fs::write(slot_path.join(".specify/project.yaml"), "name: remote\nadapter: omnia@v1\n")
        .unwrap();
    fs::write(slot_path.join("README.md"), "# remote\n").unwrap();
    run_git(&slot_path, &["init"]);
    run_git(&slot_path, &["remote", "add", "origin", remote_url]);
    run_git(&slot_path, &["add", "."]);
    run_git(&slot_path, &["commit", "-m", "initial"]);
    run_git(&slot_path, &["checkout", "-b", "feature/work"]);
    fs::write(slot_path.join("dirty.txt"), "dirty\n").unwrap();
    fs::write(project_dir.join("plan.yaml"), "name: demo-change\nslices: []\n").unwrap();
    let project = RegistryProject {
        name: "remote".to_string(),
        url: remote_url.to_string(),
        adapter: Some("omnia@v1".to_string()),
        description: Some("remote service".to_string()),
        contracts: None,
    };

    let slots = workspace_status_projects(project_dir, &[&project]);
    let slot = &slots[0];

    assert_eq!(slot.kind, SlotKind::GitClone);
    assert_eq!(
        slot.configured_target_kind,
        specify_workflow::registry::workspace::ConfiguredTargetKind::Remote
    );
    assert_eq!(slot.configured_target, remote_url);
    assert_eq!(slot.actual_origin.as_deref(), Some(remote_url));
    assert_eq!(slot.current_branch.as_deref(), Some("feature/work"));
    assert_eq!(slot.dirty, Some(true));
    assert_eq!(slot.branch_matches_change, Some(false));
    assert!(slot.project_config_present);
    assert_eq!(slot.active_slices, ["draft"]);
}

#[test]
fn c03_status_other_materialisation() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    let slot_path = project_dir.join(".specify/workspace/odd");
    fs::create_dir_all(slot_path.join(".specify/slices/manual")).unwrap();
    fs::write(slot_path.join(".specify/project.yaml"), "name: odd\nadapter: omnia@v1\n").unwrap();
    let project = RegistryProject {
        name: "odd".to_string(),
        url: "https://example.invalid/org/odd.git".to_string(),
        adapter: Some("omnia@v1".to_string()),
        description: Some("odd service".to_string()),
        contracts: None,
    };

    let slots = workspace_status_projects(project_dir, &[&project]);

    assert_eq!(slots[0].kind, SlotKind::Other);
    assert!(slots[0].project_config_present);
    assert_eq!(slots[0].active_slices, ["manual"]);
    assert_eq!(slots[0].dirty, None);
}

// ---------- workspace push (workspace orchestration contract C07) -------------------------------

#[test]
fn c07_push_pushes_clean_change_branch_only() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    let local_head = prepare_change_branch(&slot, "demo-change");
    let project = remote_project(remote_url.clone());

    let results =
        push_projects(&project_dir, "demo-change", &[&project], false).expect("push succeeds");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, PushOutcome::Pushed);
    assert_eq!(results[0].branch.as_deref(), Some("specify/demo-change"));
    assert_eq!(
        remote_branch_head(&remote_url, "specify/demo-change").as_deref(),
        Some(local_head.as_str())
    );
    assert_eq!(current_branch(&slot), "specify/demo-change", "push must not rebrand HEAD");
}

#[test]
fn c07_push_up_to_date() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    let local_head = prepare_change_branch(&slot, "demo-change");
    let project = remote_project(remote_url.clone());
    push_projects(&project_dir, "demo-change", &[&project], false).expect("initial push");

    let results =
        push_projects(&project_dir, "demo-change", &[&project], false).expect("second push");

    assert_eq!(results[0].status, PushOutcome::UpToDate);
    assert_eq!(
        remote_branch_head(&remote_url, "specify/demo-change").as_deref(),
        Some(local_head.as_str())
    );
}

#[test]
fn c07_push_dirty_without_push() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    prepare_change_branch(&slot, "demo-change");
    fs::write(slot.join("scratch.txt"), "dirty\n").unwrap();
    let project = remote_project(remote_url.clone());

    let results = push_projects(&project_dir, "demo-change", &[&project], false)
        .expect("best-effort push returns results");

    assert_eq!(results[0].status, PushOutcome::Failed);
    assert!(
        results[0].error.as_deref().is_some_and(|err| err.contains("dirty")),
        "dirty failure should be actionable: {:?}",
        results[0].error
    );
    assert!(
        remote_branch_head(&remote_url, "specify/demo-change").is_none(),
        "dirty checkouts must not be pushed"
    );
}

#[test]
fn c07_push_wrong_branch_no_checkout() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    run_git(&slot, &["checkout", "-b", "feature/work"]);
    let project = remote_project(remote_url.clone());

    let results = push_projects(&project_dir, "demo-change", &[&project], false)
        .expect("best-effort push returns results");

    assert_eq!(results[0].status, PushOutcome::NoBranch);
    assert_eq!(current_branch(&slot), "feature/work", "push must not checkout another branch");
    assert!(remote_branch_head(&remote_url, "specify/demo-change").is_none());
}

#[test]
fn c07_push_missing_origin_is_local_only() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    let slot = project_dir.join(".specify/workspace/alpha");
    fs::create_dir_all(&slot).unwrap();
    run_git(&slot, &["init", "-b", "main"]);
    fs::write(slot.join("README.md"), "seed\n").unwrap();
    run_git(&slot, &["add", "README.md"]);
    run_git(&slot, &["commit", "--no-gpg-sign", "-m", "seed"]);
    prepare_change_branch(&slot, "demo-change");
    let project = remote_project("https://example.invalid/org/alpha.git".to_string());

    let results = push_projects(&project_dir, "demo-change", &[&project], false)
        .expect("best-effort push returns results");

    assert_eq!(results[0].status, PushOutcome::LocalOnly);
}

#[test]
fn c07_push_dry_run_classifies_without_pushing() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("workspace");
    fs::create_dir_all(&project_dir).unwrap();
    let (_remote, remote_url) = seed_bare_remote(&tmp);
    let slot = clone_workspace_slot(&project_dir, &remote_url);
    prepare_change_branch(&slot, "demo-change");
    let project = remote_project(remote_url.clone());

    let results =
        push_projects(&project_dir, "demo-change", &[&project], true).expect("dry-run succeeds");

    assert_eq!(results[0].status, PushOutcome::Pushed);
    assert!(
        remote_branch_head(&remote_url, "specify/demo-change").is_none(),
        "dry-run must not push"
    );
}

#[test]
fn c07_push_selector_unknown_preflight() {
    let tmp = TempDir::new().unwrap();
    let registry = registry_with_projects(&["alpha"]);

    let err = registry
        .select(&["ghost".to_string()])
        .expect_err("unknown selector must fail before workspace work");

    let msg = err.to_string();
    assert!(msg.contains("unknown project selector"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace").exists(),
        "selector preflight must happen before touching workspace paths"
    );
}

// ---------- forge:: branch matchers ----------------------------------

#[test]
fn forge_branch_matchers_round_trip() {
    assert!(is_specify_branch("specify/foo"));
    assert!(is_specify_branch("specify/platform-v2"));
    assert!(!is_specify_branch("feature/bar"));
    assert!(!is_specify_branch("specify/foo/bar"));

    assert!(branches_match("specify/foo", "specify/foo"));
    assert!(!branches_match("specify/foo", "specify/bar"));
    assert!(!branches_match("feature/foo", "specify/foo"));
}

// Sanity: the workspace base helper is private but the path it
// computes is observable through `workspace_status` (`.specify/workspace/`).
// This test pins the layout so a future refactor to the private helper
// keeps producing the same on-disk path the binary expects.
#[test]
fn sync_lays_down_workspace_under_dot_specify() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    adapter: omnia@v1
",
    )
    .unwrap();

    let registry = Registry::load(project_dir).unwrap().expect("registry present");
    workspace_sync_projects(project_dir, &registry.select(&[]).unwrap()).unwrap();
    assert!(Path::new(&project_dir.join(".specify/workspace")).is_dir());
}

// ---------- RFC-36 topology.lock regeneration ------------------------

/// Stage a materialised slot with a resolvable omnia adapter and the
/// given `project.yaml` body under `.specify/workspace/<name>/`.
fn stage_topology_slot(project_dir: &Path, name: &str, project_yaml: &str) {
    let slot_specify = project_dir.join(".specify/workspace").join(name).join(".specify");
    fs::create_dir_all(&slot_specify).unwrap();
    fs::write(slot_specify.join("project.yaml"), project_yaml).unwrap();
    let omnia_manifest = slot_specify.join(".cache/manifests/targets/omnia");
    fs::create_dir_all(&omnia_manifest).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plugins/adapters/targets/omnia/adapter.yaml");
    fs::copy(fixture, omnia_manifest.join("adapter.yaml")).unwrap();
}

#[test]
fn regenerate_topology_lock_projects_baseline_identity() {
    use specify_workflow::registry::topology::TopologyLock;
    use specify_workflow::registry::workspace::regenerate_topology_lock;

    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    // A stale `capabilities:` key is silently ignored (RFC-36); routing
    // identity is derived from the slot's baseline, not re-authored.
    stage_topology_slot(
        project_dir,
        "alpha",
        "name: alpha\nadapter: omnia@v1\ndescription: Alpha core\ncapabilities:\n  - auth\n",
    );
    let alpha_specify = project_dir.join(".specify/workspace/alpha/.specify");
    let session_dir = alpha_specify.join("specs/session");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("spec.md"),
        "### Requirement: Issue session token\n\nID: REQ-001\n\nBody.\n\n\
         ### Requirement: Revoke session\n\nID: REQ-002\n\nBody.\n",
    )
    .unwrap();
    fs::write(
        alpha_specify.join("journal.jsonl"),
        "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"event\":\"slice.archive.created\",\"payload\":{\"slice-name\":\"session\",\"touched-specs\":[\"session\"],\"outcome-summary\":\"session: 2 added\"}}\n",
    )
    .unwrap();

    // A registry member whose slot has not been materialised yet is
    // skipped, not an error.
    let registry = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "alpha".to_string(),
                url: "git@github.com:org/alpha.git".to_string(),
                adapter: None,
                description: None,
                contracts: None,
            },
            RegistryProject {
                name: "beta".to_string(),
                url: "git@github.com:org/beta.git".to_string(),
                adapter: None,
                description: None,
                contracts: None,
            },
        ],
    };

    regenerate_topology_lock(project_dir, &registry).expect("regenerate");

    let lock = TopologyLock::load(&project_dir.join(".specify/topology.lock"))
        .expect("load")
        .expect("present");
    assert_eq!(lock.projects.len(), 1, "unmaterialised beta is skipped");
    let alpha = &lock.projects[0];
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.target, "omnia@v1");
    assert_eq!(alpha.description.as_deref(), Some("Alpha core"));
    assert_eq!(alpha.surface.len(), 1);
    assert_eq!(alpha.surface[0].unit, "session");
    assert_eq!(
        alpha.surface[0].requirements,
        vec!["Issue session token".to_string(), "Revoke session".to_string()]
    );
    assert_eq!(alpha.recent, vec!["session: 2 added".to_string()]);
}

#[test]
fn regenerate_topology_lock_projects_accepted_decisions() {
    use specify_workflow::registry::topology::TopologyLock;
    use specify_workflow::registry::workspace::regenerate_topology_lock;

    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    stage_topology_slot(
        project_dir,
        "alpha",
        "name: alpha\nadapter: omnia@v1\ndescription: Alpha core\n",
    );
    let decisions_dir = project_dir.join(".specify/workspace/alpha/.specify/decisions");
    fs::create_dir_all(&decisions_dir).unwrap();
    let decision = |id: &str, slug: &str, status: &str, title: &str| {
        format!(
            "---\nid: {id}\nslug: {slug}\nstatus: {status}\nslice: s\ndate: 2026-06-02\n---\n\
             # {title}\n\n## Context\nc\n\n## Decision\nd\n\n## Consequences\ne\n"
        )
    };
    fs::write(
        decisions_dir.join("DEC-0001-use-postgres.md"),
        decision("DEC-0001", "use-postgres", "accepted", "Use PostgreSQL"),
    )
    .unwrap();
    fs::write(
        decisions_dir.join("DEC-0002-drop-redis.md"),
        decision("DEC-0002", "drop-redis", "rejected", "Drop Redis"),
    )
    .unwrap();

    let registry = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "alpha".to_string(),
            url: "git@github.com:org/alpha.git".to_string(),
            adapter: None,
            description: None,
            contracts: None,
        }],
    };

    regenerate_topology_lock(project_dir, &registry).expect("regenerate");

    let lock = TopologyLock::load(&project_dir.join(".specify/topology.lock"))
        .expect("load")
        .expect("present");
    let alpha = &lock.projects[0];
    // Only the accepted record is projected, title-only.
    assert_eq!(alpha.decisions.len(), 1);
    assert_eq!(alpha.decisions[0].id, "DEC-0001");
    assert_eq!(alpha.decisions[0].title, "Use PostgreSQL");
    assert!(alpha.decisions_more.is_none());

    // The accepted decision round-trips onto the wire under `decisions:`.
    let yaml = fs::read_to_string(project_dir.join(".specify/topology.lock")).unwrap();
    assert!(yaml.contains("DEC-0001"), "{yaml}");
}
