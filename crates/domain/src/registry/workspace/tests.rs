use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use specify_error::Error;

use super::bootstrap::bootstrap as greenfield_bootstrap;
use super::git::git_output_ok;
use super::push::remote::{current_branch, origin_head_branch};
use super::push::{PushOutcome, PushResult, WorkspacePushForge, github_slug, push_single_project};
use super::sync::{distribute_contracts, materialise_git_remote};
use crate::registry::registry::RegistryProject;

const TEST_CHANGE: &str = "demo-change";
const TEST_BRANCH: &str = "specify/demo-change";

fn run_test_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.name=Specify", "-c", "user.email=specify@example.invalid"])
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git -C {} {} failed\nstdout:\n{}\nstderr:\n{}",
        cwd.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_test_git_dir(git_dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .output()
        .expect("spawn git --git-dir");
    assert!(
        output.status.success(),
        "git --git-dir {} {} failed\nstdout:\n{}\nstderr:\n{}",
        git_dir.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_dir_output(git_dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .output()
        .expect("spawn git --git-dir");
    assert!(
        output.status.success(),
        "git --git-dir {} {} failed: {}",
        git_dir.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_output(tree: &Path, args: &[&str]) -> String {
    git_output_ok(tree, args).unwrap_or_else(|| {
        panic!("git -C {} {} produced no stdout", tree.display(), args.join(" "))
    })
}

fn git_output_allow_empty(tree: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(tree).args(args).output().expect("spawn git");
    assert!(
        output.status.success(),
        "git -C {} {} failed: {}",
        tree.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo_with_commit(path: &Path, body: &str) {
    std::fs::create_dir_all(path).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["init", "-b", "main"])
        .output()
        .expect("spawn git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(path.join("README.md"), body).unwrap();
    run_test_git(path, &["add", "README.md"]);
    run_test_git(path, &["commit", "--no-gpg-sign", "-m", "seed"]);
}

fn test_project(url: impl Into<String>) -> RegistryProject {
    RegistryProject {
        name: "alpha".to_string(),
        url: url.into(),
        capability: "omnia@v1".to_string(),
        description: Some("alpha service".to_string()),
        contracts: None,
    }
}

fn seed_bare_remote(remote: &Path) {
    let source = remote.with_extension("source");
    init_repo_with_commit(&source, "base\n");
    run_test_git(
        remote.parent().expect("remote parent"),
        &["clone", "--bare", source.to_str().unwrap(), remote.to_str().unwrap()],
    );
}

fn clone_alpha_slot(project_dir: &Path, remote_url: &str) -> PathBuf {
    let slot = project_dir.join(".specify/workspace/alpha");
    std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
    run_test_git(project_dir, &["clone", remote_url, slot.to_str().unwrap()]);
    slot
}

fn commit_on_change_branch(worktree: &Path, file: &str, body: &str) {
    run_test_git(worktree, &["checkout", "-b", TEST_BRANCH]);
    std::fs::write(worktree.join(file), body).unwrap();
    run_test_git(worktree, &["add", file]);
    run_test_git(worktree, &["commit", "--no-gpg-sign", "-m", "change work"]);
}

fn push_alpha(
    project_dir: &Path, project: &RegistryProject, dry_run: bool, forge: &dyn WorkspacePushForge,
) -> PushResult {
    let workspace_base = project_dir.join(".specify/workspace");
    push_single_project(
        project_dir,
        &workspace_base,
        project,
        TEST_BRANCH,
        TEST_CHANGE,
        dry_run,
        forge,
    )
}

struct RecordingForge {
    repo_exists_result: bool,
    create_remote: Option<PathBuf>,
    repo_exists_calls: RefCell<Vec<String>>,
    create_repo_calls: RefCell<Vec<String>>,
    pr_calls: RefCell<Vec<(String, String, String)>>,
}

impl RecordingForge {
    fn new(repo_exists_result: bool) -> Self {
        Self {
            repo_exists_result,
            create_remote: None,
            repo_exists_calls: RefCell::new(Vec::new()),
            create_repo_calls: RefCell::new(Vec::new()),
            pr_calls: RefCell::new(Vec::new()),
        }
    }

    fn creating(remote: PathBuf) -> Self {
        Self {
            create_remote: Some(remote),
            ..Self::new(false)
        }
    }
}

impl WorkspacePushForge for RecordingForge {
    fn repo_exists(&self, slug: &str, _project_path: &Path) -> Result<bool, Error> {
        self.repo_exists_calls.borrow_mut().push(slug.to_string());
        Ok(self.repo_exists_result)
    }

    fn create_repo(&self, slug: &str, _project_path: &Path) -> Result<(), Error> {
        self.create_repo_calls.borrow_mut().push(slug.to_string());
        if let Some(remote) = &self.create_remote {
            seed_bare_remote(remote);
        }
        Ok(())
    }

    fn ensure_pull_request(
        &self, _project_path: &Path, branch_name: &str, base_branch: &str, change_name: &str,
    ) -> Result<u64, Error> {
        self.pr_calls.borrow_mut().push((
            branch_name.to_string(),
            base_branch.to_string(),
            change_name.to_string(),
        ));
        Ok(42)
    }
}

#[test]
fn rfc14_c07_workspace_push_publishes_existing_change_branch_only() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    commit_on_change_branch(&slot, "change.txt", "work\n");
    let local_head = git_output(&slot, &["rev-parse", "HEAD"]);
    let commits_before = git_output(&slot, &["rev-list", "--count", "HEAD"]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::Pushed);
    assert_eq!(result.branch.as_deref(), Some(TEST_BRANCH));
    assert_eq!(current_branch(&slot).unwrap().as_deref(), Some(TEST_BRANCH));
    assert_eq!(git_output(&slot, &["rev-list", "--count", "HEAD"]), commits_before);
    assert_eq!(git_output(&remote, &["rev-parse", TEST_BRANCH]), local_head);
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_reports_up_to_date_without_pushing() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    commit_on_change_branch(&slot, "change.txt", "work\n");
    run_test_git(&slot, &["push", "origin", TEST_BRANCH]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::UpToDate);
    assert_eq!(result.branch.as_deref(), Some(TEST_BRANCH));
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_dirty_checkout_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    commit_on_change_branch(&slot, "change.txt", "work\n");
    std::fs::write(slot.join("dirty.txt"), "dirty\n").unwrap();
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::Failed);
    assert!(result.error.as_deref().is_some_and(|error| error.contains("dirty")));
    assert!(forge.repo_exists_calls.borrow().is_empty());
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_wrong_branch_is_no_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    run_test_git(&slot, &["checkout", "-b", "feature/not-the-change"]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert_eq!(current_branch(&slot).unwrap().as_deref(), Some("feature/not-the-change"));
    assert!(forge.repo_exists_calls.borrow().is_empty());
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_default_branch_is_no_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    commit_on_change_branch(&slot, "change.txt", "work\n");
    run_test_git(&slot, &["push", "origin", TEST_BRANCH]);
    run_test_git(&remote, &["symbolic-ref", "HEAD", &format!("refs/heads/{TEST_BRANCH}")]);
    run_test_git(&slot, &["remote", "set-head", "origin", "--auto"]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_detached_head_is_no_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    let head = git_output(&slot, &["rev-parse", "HEAD"]);
    run_test_git(&slot, &["checkout", "--detach", &head]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert!(current_branch(&slot).unwrap().is_none());
}

#[test]
fn rfc14_c07_workspace_push_refuses_remote_default_branch_as_no_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    commit_on_change_branch(&slot, "change.txt", "work\n");
    run_test_git(&slot, &["push", "origin", TEST_BRANCH]);
    run_test_git_dir(&remote, &["symbolic-ref", "HEAD", &format!("refs/heads/{TEST_BRANCH}")]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &forge);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_local_only_without_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path();
    let alpha = project_dir.join("alpha");
    init_repo_with_commit(&alpha, "seed\n");
    run_test_git(&alpha, &["checkout", "-b", TEST_BRANCH]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(project_dir, &test_project("./alpha"), false, &forge);

    assert_eq!(result.status, PushOutcome::LocalOnly);
    assert!(result.branch.is_none());
    assert!(forge.repo_exists_calls.borrow().is_empty());
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_greenfield_creates_remote_then_pr_to_origin_head() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    let slot = project_dir.join(".specify/workspace/alpha");
    std::fs::create_dir_all(&slot).unwrap();
    run_test_git(&slot, &["init", "-b", "main"]);
    run_test_git(&slot, &["checkout", "-b", TEST_BRANCH]);
    std::fs::write(slot.join("README.md"), "greenfield\n").unwrap();
    run_test_git(&slot, &["add", "README.md"]);
    run_test_git(&slot, &["commit", "--no-gpg-sign", "-m", "greenfield work"]);
    let remote = tmp.path().join("alpha.git");
    let github_url = "https://github.com/org/alpha.git";
    let rewrite = format!("file://{}", remote.display());
    run_test_git(&slot, &["remote", "add", "origin", github_url]);
    run_test_git(&slot, &["config", &format!("url.{rewrite}.insteadOf"), github_url]);
    let forge = RecordingForge::creating(remote.clone());

    let result = push_alpha(&project_dir, &test_project(github_url), false, &forge);

    assert_eq!(result.status, PushOutcome::Created);
    assert_eq!(result.pr_number, Some(42));
    assert_eq!(forge.create_repo_calls.borrow().as_slice(), ["org/alpha"]);
    assert_eq!(
        forge.pr_calls.borrow().as_slice(),
        [(TEST_BRANCH.to_string(), "main".to_string(), TEST_CHANGE.to_string())]
    );
    assert_eq!(
        git_output(&remote, &["rev-parse", TEST_BRANCH]),
        git_output(&slot, &["rev-parse", "HEAD"])
    );
}

#[test]
fn rfc14_c07_workspace_push_dry_run_classifies_without_mutating_remote_or_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    std::fs::create_dir_all(&project_dir).unwrap();
    let remote = tmp.path().join("alpha.git");
    seed_bare_remote(&remote);
    let remote_url = format!("file://{}", remote.display());
    let slot = clone_alpha_slot(&project_dir, &remote_url);
    commit_on_change_branch(&slot, "change.txt", "work\n");
    let github_url = "https://github.com/org/alpha.git";
    let rewrite = format!("file://{}", remote.display());
    run_test_git(&slot, &["remote", "set-url", "origin", github_url]);
    run_test_git(&slot, &["config", &format!("url.{rewrite}.insteadOf"), github_url]);
    let forge = RecordingForge::new(true);

    let result = push_alpha(&project_dir, &test_project(github_url), true, &forge);

    assert_eq!(result.status, PushOutcome::Pushed);
    assert!(git_output_ok(&remote, &["rev-parse", TEST_BRANCH]).is_none());
    assert!(forge.create_repo_calls.borrow().is_empty());
    assert!(forge.pr_calls.borrow().is_empty());
}

#[test]
fn extract_github_slug_git_ssh() {
    assert_eq!(github_slug("git@github.com:org/mobile.git"), Some("org/mobile".to_string()));
}

#[test]
fn extract_github_slug_git_ssh_no_suffix() {
    assert_eq!(github_slug("git@github.com:org/mobile"), Some("org/mobile".to_string()));
}

#[test]
fn extract_github_slug_https() {
    assert_eq!(github_slug("https://github.com/org/mobile.git"), Some("org/mobile".to_string()));
}

#[test]
fn extract_github_slug_https_no_suffix() {
    assert_eq!(github_slug("https://github.com/org/mobile"), Some("org/mobile".to_string()));
}

#[test]
fn extract_github_slug_ssh_protocol() {
    assert_eq!(github_slug("ssh://git@github.com/org/mobile.git"), Some("org/mobile".to_string()));
}

#[test]
fn extract_github_slug_non_github() {
    assert_eq!(github_slug("git@gitlab.com:org/repo.git"), None);
}

#[test]
fn distribute_contracts_recursive() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("contracts");
    let nested = src.join("schemas");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(src.join("openapi.yaml"), "openapi: 3.1").unwrap();
    std::fs::write(nested.join("order.yaml"), "type: object").unwrap();

    let dest = tmp.path().join("slot").join("contracts");
    distribute_contracts(&src, &dest).unwrap();

    assert!(dest.join("openapi.yaml").is_file());
    assert_eq!(std::fs::read_to_string(dest.join("openapi.yaml")).unwrap(), "openapi: 3.1");
    assert!(dest.join("schemas").join("order.yaml").is_file());
    assert_eq!(
        std::fs::read_to_string(dest.join("schemas").join("order.yaml")).unwrap(),
        "type: object"
    );
}

#[test]
fn distribute_contracts_replaces_dest() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("contracts");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("v2.yaml"), "version: 2").unwrap();

    let dest = tmp.path().join("dest_contracts");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.yaml"), "old").unwrap();

    distribute_contracts(&src, &dest).unwrap();

    assert!(dest.join("v2.yaml").is_file());
    assert!(!dest.join("stale.yaml").exists(), "stale file should be removed");
}

#[test]
fn distribute_contracts_missing_src_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("does-not-exist");
    let dest = tmp.path().join("dest");

    assert!(!src.is_dir());
    assert!(!dest.exists());
}

#[test]
fn rfc14_c02_remote_clone_fetches_existing_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote-source");
    init_repo_with_commit(&remote, "v1\n");
    std::fs::create_dir_all(remote.join(".specify")).unwrap();
    std::fs::write(remote.join(".specify/project.yaml"), "name: remote\ncapability: omnia\n")
        .unwrap();
    run_test_git(&remote, &["add", ".specify/project.yaml"]);
    run_test_git(&remote, &["commit", "--no-gpg-sign", "-m", "add specify config"]);
    let url = format!("file://{}", remote.display());
    let dest = tmp.path().join(".specify/workspace/remote");

    materialise_git_remote(&url, &dest, "https://example.invalid/capability", tmp.path())
        .expect("initial clone");
    let initial_origin_main = git_output(&dest, &["rev-parse", "origin/main"]);
    assert_eq!(initial_origin_main, git_output(&remote, &["rev-parse", "HEAD"]));

    std::fs::write(remote.join("README.md"), "v2\n").unwrap();
    run_test_git(&remote, &["add", "README.md"]);
    run_test_git(&remote, &["commit", "--no-gpg-sign", "-m", "update"]);
    let updated_head = git_output(&remote, &["rev-parse", "HEAD"]);

    materialise_git_remote(&url, &dest, "https://example.invalid/capability", tmp.path())
        .expect("fetch existing clone");

    assert_ne!(initial_origin_main, updated_head);
    assert_eq!(git_output(&dest, &["rev-parse", "origin/main"]), updated_head);
}

#[test]
fn rfc14_c02_remote_clone_refuses_origin_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join(".specify/workspace/remote");
    init_repo_with_commit(&dest, "slot\n");
    run_test_git(&dest, &["remote", "add", "origin", "https://example.invalid/old.git"]);
    std::fs::create_dir_all(dest.join(".specify")).unwrap();
    std::fs::write(dest.join(".specify/project.yaml"), "name: remote\ncapability: omnia\n")
        .unwrap();

    let err = materialise_git_remote(
        "https://example.invalid/new.git",
        &dest,
        "https://example.invalid/capability",
        tmp.path(),
    )
    .expect_err("origin mismatch must fail");
    let msg = err.to_string();

    assert!(msg.contains("origin remote"), "msg: {msg}");
    assert!(msg.contains("https://example.invalid/old.git"), "msg: {msg}");
    assert!(msg.contains("https://example.invalid/new.git"), "msg: {msg}");
}

#[test]
fn rfc14_c02_greenfield_bootstrap_stays_local_and_commits_scaffold() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join(".specify/workspace/new-service");
    let url = "https://example.invalid/org/new-service.git";

    greenfield_bootstrap(url, &dest, "https://example.invalid/capability", tmp.path())
        .expect("greenfield bootstrap");

    assert_eq!(git_output(&dest, &["remote", "get-url", "origin"]), url);
    let project_yaml = std::fs::read_to_string(dest.join(".specify/project.yaml")).unwrap();
    assert!(project_yaml.contains("name: new-service"), "{project_yaml}");
    assert!(project_yaml.contains("capability: https://example.invalid/capability"));
    assert!(git_output_ok(&dest, &["log", "--oneline", "-1"]).is_some());
    assert_eq!(git_output_allow_empty(&dest, &["status", "--porcelain"]), "");
}

struct FakePushForge {
    repo_exists: bool,
    remote_to_create: Option<PathBuf>,
    branch_absent_after_create: Option<String>,
    pr_calls: std::cell::RefCell<Vec<(String, String)>>,
}

impl FakePushForge {
    fn new(repo_exists: bool) -> Self {
        Self {
            repo_exists,
            remote_to_create: None,
            branch_absent_after_create: None,
            pr_calls: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn creating(mut self, remote: PathBuf, absent_branch: &str) -> Self {
        self.remote_to_create = Some(remote);
        self.branch_absent_after_create = Some(absent_branch.to_string());
        self
    }
}

impl WorkspacePushForge for FakePushForge {
    fn repo_exists(&self, _slug: &str, _project_path: &Path) -> Result<bool, Error> {
        Ok(self.repo_exists)
    }

    fn create_repo(&self, _slug: &str, project_path: &Path) -> Result<(), Error> {
        let Some(remote) = &self.remote_to_create else {
            return Ok(());
        };
        run_test_git(
            remote.parent().unwrap(),
            &["clone", "--bare", project_path.to_str().unwrap(), remote.to_str().unwrap()],
        );
        if let Some(branch) = &self.branch_absent_after_create {
            run_test_git_dir(remote, &["update-ref", "-d", &format!("refs/heads/{branch}")]);
        }
        run_test_git_dir(remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        Ok(())
    }

    fn ensure_pull_request(
        &self, _project_path: &Path, branch_name: &str, base_branch: &str, _initiative_name: &str,
    ) -> Result<u64, Error> {
        self.pr_calls.borrow_mut().push((branch_name.to_string(), base_branch.to_string()));
        Ok(42)
    }
}

#[test]
fn rfc14_c07_greenfield_push_creates_repo_then_pr_against_origin_head() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    let workspace_base = project_dir.join(".specify/workspace");
    let slot = workspace_base.join("alpha");
    init_repo_with_commit(&slot, "seed\n");
    let main_head = git_output(&slot, &["rev-parse", "main"]);
    run_test_git(&slot, &["checkout", "-b", "specify/demo-change"]);
    std::fs::write(slot.join("change.txt"), "work\n").unwrap();
    run_test_git(&slot, &["add", "change.txt"]);
    run_test_git(&slot, &["commit", "--no-gpg-sign", "-m", "change work"]);
    let change_head = git_output(&slot, &["rev-parse", "HEAD"]);

    let github_url = "https://github.com/org/alpha.git";
    let remote = tmp.path().join("alpha.git");
    run_test_git(&slot, &["remote", "add", "origin", github_url]);
    run_test_git(
        &slot,
        &["config", &format!("url.file://{}.insteadOf", remote.display()), github_url],
    );
    let project = RegistryProject {
        name: "alpha".to_string(),
        url: github_url.to_string(),
        capability: "omnia@v1".to_string(),
        description: Some("alpha service".to_string()),
        contracts: None,
    };
    let forge = FakePushForge::new(false).creating(remote.clone(), "specify/demo-change");

    let result = push_single_project(
        &project_dir,
        &workspace_base,
        &project,
        "specify/demo-change",
        "demo-change",
        false,
        &forge,
    );

    assert_eq!(result.status, PushOutcome::Created, "result: {result:?}");
    assert_eq!(result.pr_number, Some(42));
    assert_eq!(
        git_dir_output(&remote, &["rev-parse", "refs/heads/specify/demo-change"]),
        change_head
    );
    assert_eq!(git_dir_output(&remote, &["rev-parse", "refs/heads/main"]), main_head);
    assert_eq!(origin_head_branch(&slot).as_deref(), Some("main"));
    assert_eq!(
        forge.pr_calls.borrow().as_slice(),
        &[("specify/demo-change".to_string(), "main".to_string())]
    );
}

#[test]
fn rfc14_c07_greenfield_dry_run_does_not_create_repo_or_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path().join("hub");
    let workspace_base = project_dir.join(".specify/workspace");
    let slot = workspace_base.join("alpha");
    init_repo_with_commit(&slot, "seed\n");
    run_test_git(&slot, &["checkout", "-b", "specify/demo-change"]);
    std::fs::write(slot.join("change.txt"), "work\n").unwrap();
    run_test_git(&slot, &["add", "change.txt"]);
    run_test_git(&slot, &["commit", "--no-gpg-sign", "-m", "change work"]);

    let github_url = "https://github.com/org/alpha.git";
    let remote = tmp.path().join("alpha.git");
    run_test_git(&slot, &["remote", "add", "origin", github_url]);
    run_test_git(
        &slot,
        &["config", &format!("url.file://{}.insteadOf", remote.display()), github_url],
    );
    let project = RegistryProject {
        name: "alpha".to_string(),
        url: github_url.to_string(),
        capability: "omnia@v1".to_string(),
        description: Some("alpha service".to_string()),
        contracts: None,
    };
    let forge = FakePushForge::new(false).creating(remote.clone(), "specify/demo-change");

    let result = push_single_project(
        &project_dir,
        &workspace_base,
        &project,
        "specify/demo-change",
        "demo-change",
        true,
        &forge,
    );

    assert_eq!(result.status, PushOutcome::Created);
    assert!(!remote.exists(), "dry-run must not create the remote repository");
    assert!(forge.pr_calls.borrow().is_empty(), "dry-run must not create or update PRs");
}
