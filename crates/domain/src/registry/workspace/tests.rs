use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};
use std::rc::Rc;

use super::bootstrap::bootstrap as greenfield_bootstrap;
use super::git::git_output_ok;
use super::push::remote::{current_branch, origin_head_branch};
use super::push::{PushOutcome, PushResult, github_slug, push_single_project};
use super::sync::{distribute_contracts, materialise_git_remote};
use crate::cmd::CmdRunner;
use crate::registry::catalog::RegistryProject;

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

fn push_alpha<R: CmdRunner>(
    project_dir: &Path, project: &RegistryProject, dry_run: bool, runner: &R,
) -> PushResult {
    let workspace_base = project_dir.join(".specify/workspace");
    push_single_project(
        project_dir,
        &workspace_base,
        project,
        TEST_BRANCH,
        TEST_CHANGE,
        dry_run,
        runner,
    )
}

// --- MockCmd: a CmdRunner that intercepts the `gh` shell-outs --------
//
// Real `git` invocations still hit the filesystem (these tests rely on
// real repositories under tempdirs). Only `gh repo view`, `gh repo
// create`, `gh pr list`, `gh pr edit`, and `gh pr create` are mocked.

fn exit_success() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

fn exit_failure() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(1 << 8)
}

fn ok_stdout(stdout: &str) -> io::Result<Output> {
    Ok(Output {
        status: exit_success(),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    })
}

fn fail_stderr(stderr: &str) -> io::Result<Output> {
    Ok(Output {
        status: exit_failure(),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

#[derive(Clone, Default)]
struct ForgeRecord {
    repo_exists_calls: Rc<RefCell<Vec<String>>>,
    create_repo_calls: Rc<RefCell<Vec<String>>>,
    pr_calls: Rc<RefCell<Vec<(String, String, String)>>>,
}

/// `gh`-only mock. Real `git` is allowed to run.
struct GhMock {
    record: ForgeRecord,
    inner: RefCell<GhMockInner>,
}

struct GhMockInner {
    repo_exists_result: bool,
    on_create_repo: Option<Box<dyn FnMut(&Path)>>,
}

impl GhMock {
    fn new(repo_exists_result: bool) -> Self {
        Self {
            record: ForgeRecord::default(),
            inner: RefCell::new(GhMockInner {
                repo_exists_result,
                on_create_repo: None,
            }),
        }
    }

    fn with_create<F>(self, callback: F) -> Self
    where
        F: FnMut(&Path) + 'static,
    {
        self.inner.borrow_mut().on_create_repo = Some(Box::new(callback));
        self
    }

    fn record(&self) -> ForgeRecord {
        self.record.clone()
    }
}

impl CmdRunner for GhMock {
    fn run(&self, cmd: &mut Command) -> io::Result<Output> {
        let program = cmd.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let cwd = cmd.get_current_dir().map(PathBuf::from);

        // Pass real git through.
        if program != "gh" {
            return cmd.output();
        }

        match args.as_slice() {
            // gh repo view <slug> --json name
            [first, second, slug, ..] if first == "repo" && second == "view" => {
                self.record.repo_exists_calls.borrow_mut().push(slug.clone());
                if self.inner.borrow().repo_exists_result {
                    ok_stdout("{\"name\":\"alpha\"}\n")
                } else {
                    fail_stderr(&format!(
                        "GraphQL: Could not resolve to a Repository with the name '{slug}'."
                    ))
                }
            }
            // gh repo create <slug> --private --source .
            [first, second, slug, ..] if first == "repo" && second == "create" => {
                self.record.create_repo_calls.borrow_mut().push(slug.clone());
                if let Some(cb) = self.inner.borrow_mut().on_create_repo.as_mut() {
                    let project_path = cwd.expect("gh repo create must set current_dir");
                    cb(&project_path);
                }
                ok_stdout("")
            }
            // gh pr list --head <branch> ...
            [first, second, ..] if first == "pr" && second == "list" => {
                // No existing PR in these tests.
                ok_stdout("[]\n")
            }
            // gh pr edit <number> --base <branch>
            [first, second, ..] if first == "pr" && second == "edit" => ok_stdout(""),
            // gh pr create --base <base> --head <branch> --title ... --body ...
            args if args.first().map(String::as_str) == Some("pr")
                && args.get(1).map(String::as_str) == Some("create") =>
            {
                let base = pick_flag(args, "--base").unwrap_or_default();
                let head = pick_flag(args, "--head").unwrap_or_default();
                self.record.pr_calls.borrow_mut().push((head, base, TEST_CHANGE.to_string()));
                // Stdout is the PR URL — `forge::ensure_pull_request`
                // parses the trailing number after `rsplit('/')`.
                ok_stdout("https://github.com/org/alpha/pull/42\n")
            }
            other => panic!("unexpected gh invocation: {other:?}"),
        }
    }
}

fn pick_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

    assert_eq!(result.status, PushOutcome::Pushed);
    assert_eq!(result.branch.as_deref(), Some(TEST_BRANCH));
    assert_eq!(current_branch(&slot).unwrap().as_deref(), Some(TEST_BRANCH));
    assert_eq!(git_output(&slot, &["rev-list", "--count", "HEAD"]), commits_before);
    assert_eq!(git_output(&remote, &["rev-parse", TEST_BRANCH]), local_head);
    assert!(record.pr_calls.borrow().is_empty());
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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

    assert_eq!(result.status, PushOutcome::UpToDate);
    assert_eq!(result.branch.as_deref(), Some(TEST_BRANCH));
    assert!(record.pr_calls.borrow().is_empty());
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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

    assert_eq!(result.status, PushOutcome::Failed);
    assert!(result.error.as_deref().is_some_and(|error| error.contains("dirty")));
    assert!(record.repo_exists_calls.borrow().is_empty());
    assert!(record.pr_calls.borrow().is_empty());
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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert_eq!(current_branch(&slot).unwrap().as_deref(), Some("feature/not-the-change"));
    assert!(record.repo_exists_calls.borrow().is_empty());
    assert!(record.pr_calls.borrow().is_empty());
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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert!(record.pr_calls.borrow().is_empty());
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
    let runner = GhMock::new(true);

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(remote_url), false, &runner);

    assert_eq!(result.status, PushOutcome::NoBranch);
    assert!(record.pr_calls.borrow().is_empty());
}

#[test]
fn rfc14_c07_workspace_push_local_only_without_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path();
    let alpha = project_dir.join("alpha");
    init_repo_with_commit(&alpha, "seed\n");
    run_test_git(&alpha, &["checkout", "-b", TEST_BRANCH]);
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(project_dir, &test_project("./alpha"), false, &runner);

    assert_eq!(result.status, PushOutcome::LocalOnly);
    assert!(result.branch.is_none());
    assert!(record.repo_exists_calls.borrow().is_empty());
    assert!(record.pr_calls.borrow().is_empty());
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
    let remote_for_cb = remote.clone();
    let runner = GhMock::new(false).with_create(move |_| {
        seed_bare_remote(&remote_for_cb);
    });
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(github_url), false, &runner);

    assert_eq!(result.status, PushOutcome::Created);
    assert_eq!(result.pr_number, Some(42));
    assert_eq!(record.create_repo_calls.borrow().as_slice(), ["org/alpha"]);
    assert_eq!(
        record.pr_calls.borrow().as_slice(),
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
    let runner = GhMock::new(true);
    let record = runner.record();

    let result = push_alpha(&project_dir, &test_project(github_url), true, &runner);

    assert_eq!(result.status, PushOutcome::Pushed);
    assert!(git_output_ok(&remote, &["rev-parse", TEST_BRANCH]).is_none());
    assert!(record.create_repo_calls.borrow().is_empty());
    assert!(record.pr_calls.borrow().is_empty());
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
    let remote_for_cb = remote.clone();
    let absent_branch = "specify/demo-change".to_string();
    let runner = GhMock::new(false).with_create(move |project_path| {
        run_test_git(
            remote_for_cb.parent().unwrap(),
            &["clone", "--bare", project_path.to_str().unwrap(), remote_for_cb.to_str().unwrap()],
        );
        run_test_git_dir(
            &remote_for_cb,
            &["update-ref", "-d", &format!("refs/heads/{absent_branch}")],
        );
        run_test_git_dir(&remote_for_cb, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    });
    let record = runner.record();

    let result = push_single_project(
        &project_dir,
        &workspace_base,
        &project,
        "specify/demo-change",
        "demo-change",
        false,
        &runner,
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
        record.pr_calls.borrow().as_slice(),
        &[("specify/demo-change".to_string(), "main".to_string(), TEST_CHANGE.to_string())]
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
    let runner = GhMock::new(false);
    let record = runner.record();

    let result = push_single_project(
        &project_dir,
        &workspace_base,
        &project,
        "specify/demo-change",
        "demo-change",
        true,
        &runner,
    );

    assert_eq!(result.status, PushOutcome::Created);
    assert!(!remote.exists(), "dry-run must not create the remote repository");
    assert!(record.pr_calls.borrow().is_empty(), "dry-run must not create or update PRs");
}
