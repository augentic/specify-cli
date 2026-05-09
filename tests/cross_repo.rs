//! Replayable acceptance coverage for the RM-01 cross-repo happy path.
//!
//! The workflow skills remain agent-driven, so this test exercises the
//! deterministic CLI substrate they compose: hub setup, registry routing,
//! plan lifecycle, workspace sync/branch preparation, push handoff, external
//! PR merge observation, and finalization.

#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::{env, fs};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

const CHANGE_NAME: &str = "oauth-login";
const BRANCH_NAME: &str = "specify/oauth-login";

const GIT_TEST_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Specify Test"),
    ("GIT_AUTHOR_EMAIL", "specify-test@example.com"),
    ("GIT_COMMITTER_NAME", "Specify Test"),
    ("GIT_COMMITTER_EMAIL", "specify-test@example.com"),
];

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn run_git(root: &Path, args: &[&str], envs: &TestEnv) -> String {
    let output = ProcessCommand::new("git")
        .current_dir(root)
        .args(args)
        .envs(GIT_TEST_ENV)
        .env("GIT_CONFIG_GLOBAL", &envs.git_config)
        .env("GIT_SSH_COMMAND", &envs.ssh_script)
        .env("FAKE_GITHUB_REMOTE_ROOT", envs.remotes_dir())
        .output()
        .unwrap_or_else(|err| panic!("git {} failed to start: {err}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed in {}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout utf8")
}

fn git_output(root: &Path, args: &[&str], envs: &TestEnv) -> String {
    run_git(root, args, envs).trim().to_string()
}

fn parse_json(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"))
}

struct TestEnv {
    _tmp: TempDir,
    root: PathBuf,
    bin_dir: PathBuf,
    gh_state: PathBuf,
    git_config: PathBuf,
    ssh_script: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let bin_dir = root.join("bin");
        let gh_state = root.join("gh-state");
        fs::create_dir_all(&bin_dir).expect("mkdir bin");
        fs::create_dir_all(&gh_state).expect("mkdir gh-state");

        let remotes = root.join("remotes");
        fs::create_dir_all(&remotes).expect("mkdir remotes");
        let git_config = root.join("gitconfig");
        fs::write(&git_config, "").expect("write gitconfig");
        let ssh_script = bin_dir.join("fake-ssh");

        let envs = Self {
            _tmp: tmp,
            root,
            bin_dir,
            gh_state,
            git_config,
            ssh_script,
        };
        envs.write_fake_gh();
        envs.write_fake_ssh();
        envs
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn remotes_dir(&self) -> PathBuf {
        self.root.join("remotes")
    }

    fn command(&self) -> Command {
        let mut cmd = specify();
        cmd.current_dir(self.path())
            .envs(GIT_TEST_ENV)
            .env("GIT_CONFIG_GLOBAL", &self.git_config)
            .env("GIT_SSH_COMMAND", &self.ssh_script)
            .env("FAKE_GITHUB_REMOTE_ROOT", self.remotes_dir())
            .env("GH_STATE_DIR", &self.gh_state)
            .env("PATH", path_with_front(&self.bin_dir));
        cmd
    }

    fn write_fake_gh(&self) {
        let script = self.bin_dir.join("gh");
        fs::write(&script, FAKE_GH).expect("write fake gh");
        let mut perms = fs::metadata(&script).expect("fake gh metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod fake gh");
    }

    fn write_fake_ssh(&self) {
        fs::write(&self.ssh_script, FAKE_SSH).expect("write fake ssh");
        let mut perms = fs::metadata(&self.ssh_script).expect("fake ssh metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&self.ssh_script, perms).expect("chmod fake ssh");
    }

    fn mark_all_prs_merged(&self) {
        for entry in fs::read_dir(&self.gh_state).expect("read gh state") {
            let entry = entry.expect("gh state entry");
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("pr") {
                continue;
            }
            let contents = fs::read_to_string(entry.path()).expect("read pr state");
            let fields: Vec<&str> = contents.trim_end().split('|').collect();
            assert_eq!(fields.len(), 5, "unexpected PR state shape: {contents}");
            fs::write(
                entry.path(),
                format!("{}|MERGED|true|{}|{}\n", fields[0], fields[3], fields[4]),
            )
            .expect("mark pr merged");
        }
    }
}

fn path_with_front(front: &Path) -> OsString {
    let mut paths = vec![front.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).expect("join PATH")
}

const FAKE_GH: &str = r#"#!/bin/sh
set -eu

state_dir="${GH_STATE_DIR:?GH_STATE_DIR is required}"
mkdir -p "$state_dir"

repo_slug() {
  url="$(git config --get remote.origin.url 2>/dev/null || true)"
  case "$url" in
    git@github.com:*) slug="${url#git@github.com:}" ;;
    https://github.com/*) slug="${url#https://github.com/}" ;;
    http://github.com/*) slug="${url#http://github.com/}" ;;
    ssh://git@github.com/*) slug="${url#ssh://git@github.com/}" ;;
    *) slug="$url" ;;
  esac
  slug="${slug%.git}"
  printf '%s\n' "$slug"
}

repo_key() {
  repo_slug | tr '/:' '__'
}

pr_file() {
  printf '%s/%s.pr\n' "$state_dir" "$(repo_key)"
}

case "${1:-}" in
  repo)
    if [ "${2:-}" = "view" ]; then
      name="$(basename "${3:-unknown}" .git)"
      printf '{"name":"%s"}\n' "$name"
      exit 0
    fi
    ;;
  pr)
    case "${2:-}" in
      list)
        file="$(pr_file)"
        if [ -f "$file" ]; then
          number="$(cut -d '|' -f 1 "$file")"
          printf '[{"number":%s}]\n' "$number"
        else
          printf '[]\n'
        fi
        exit 0
        ;;
      create)
        branch=""
        while [ "$#" -gt 0 ]; do
          case "$1" in
            --head) shift; branch="${1:-}" ;;
          esac
          shift || true
        done
        slug="$(repo_slug)"
        repo="$(basename "$slug")"
        case "$repo" in
          shop-backend) number=41 ;;
          shop-mobile) number=18 ;;
          *) number=99 ;;
        esac
        url="https://github.com/$slug/pull/$number"
        printf '%s|OPEN|false|%s|%s\n' "$number" "$branch" "$url" > "$(pr_file)"
        printf '%s\n' "$url"
        exit 0
        ;;
      edit)
        exit 0
        ;;
      view)
        file="$(pr_file)"
        if [ ! -f "$file" ]; then
          echo "no pull request" >&2
          exit 1
        fi
        IFS='|' read -r number state merged branch url < "$file"
        printf '{"state":"%s","merged":%s,"headRefName":"%s","number":%s,"url":"%s"}\n' \
          "$state" "$merged" "$branch" "$number" "$url"
        exit 0
        ;;
    esac
    ;;
esac

echo "unsupported fake gh invocation: $*" >&2
exit 1
"#;

const FAKE_SSH: &str = r#"#!/bin/sh
set -eu

remote_root="${FAKE_GITHUB_REMOTE_ROOT:?FAKE_GITHUB_REMOTE_ROOT is required}"

if [ "$#" -lt 2 ]; then
  echo "unsupported fake ssh invocation: $*" >&2
  exit 1
fi

shift
command_line="$*"
operation="${command_line%% *}"
repo_path="${command_line#* }"
repo_path="${repo_path#\'}"
repo_path="${repo_path%\'}"
repo_path="${repo_path#/}"
repo_name="${repo_path##*/}"

case "$operation" in
  git-upload-pack|git-receive-pack)
    exec "$operation" "$remote_root/$repo_name"
    ;;
esac

echo "unsupported fake ssh git operation: $operation" >&2
exit 1
"#;

struct FixtureProject {
    name: &'static str,
    slug: &'static str,
    capability: &'static str,
}

impl FixtureProject {
    fn new(envs: &TestEnv, name: &'static str, capability: &'static str) -> Self {
        let source = envs.path().join("sources").join(name);
        let remote = envs.remotes_dir().join(format!("{name}.git"));
        fs::create_dir_all(source.join(".specify")).expect("mkdir project specify");
        run_git(&source, &["init", "-b", "main"], envs);
        fs::write(source.join("README.md"), format!("# {name}\n")).expect("write README");
        fs::write(
            source.join(".specify/project.yaml"),
            format!("name: {name}\ncapability: {capability}\n"),
        )
        .expect("write project.yaml");
        run_git(&source, &["add", "."], envs);
        run_git(&source, &["commit", "--no-gpg-sign", "-m", "seed project"], envs);
        run_git(
            envs.remotes_dir().as_path(),
            &["clone", "--bare", source.to_str().unwrap(), remote.to_str().unwrap()],
            envs,
        );

        Self {
            name,
            slug: match name {
                "shop-backend" => "shop/shop-backend",
                "shop-mobile" => "shop/shop-mobile",
                _ => unreachable!("unexpected fixture project"),
            },
            capability,
        }
    }

    fn github_url(&self) -> String {
        format!("git@github.com:{}.git", self.slug)
    }
}

#[test]
fn rm01_replays_cross_repo_happy_path_through_push_and_finalize() {
    let envs = TestEnv::new();
    let backend = FixtureProject::new(&envs, "shop-backend", "omnia@v1");
    let mobile = FixtureProject::new(&envs, "shop-mobile", "vectis@v1");

    seed_hub(&envs);
    register_project(
        &envs,
        &backend,
        "User registration, account management, OAuth provider integration, token storage, and the authoritative HTTP API.",
    );
    register_project(
        &envs,
        &mobile,
        "iOS and Android mobile clients with login screens, OAuth redirect handling, and token refresh flows.",
    );
    seed_change_plan(&envs);

    assert_registry_and_plan_are_valid(&envs);
    sync_workspace(&envs);

    replay_contract_slice(&envs);
    replay_project_slice(&envs, &backend, "add-oauth-tokens", "crates/oauth_tokens/src/lib.rs");
    replay_project_slice(&envs, &mobile, "add-oauth-screens", "apps/mobile/login_screen.swift");
    assert_all_done(&envs);

    assert_workspace_ready_for_push(&envs);
    push_workspace(&envs);
    envs.mark_all_prs_merged();
    finalize_and_assert_archive(&envs);
    assert_finalize_is_idempotent(&envs);
}

fn seed_hub(envs: &TestEnv) {
    envs.command().args(["init", "--name", "shop-platform", "--hub"]).assert().success();
}

fn register_project(envs: &TestEnv, project: &FixtureProject, description: &str) {
    envs.command()
        .args([
            "registry",
            "add",
            project.name,
            "--url",
            &project.github_url(),
            "--capability",
            project.capability,
            "--description",
            description,
        ])
        .assert()
        .success();
}

fn seed_change_plan(envs: &TestEnv) {
    envs.command().args(["change", "create", CHANGE_NAME]).assert().success();
    envs.command().args(["change", "plan", "create", CHANGE_NAME]).assert().success();
    envs.command()
        .args([
            "change",
            "plan",
            "add",
            "oauth-login-contract",
            "--capability",
            "contracts@v1",
            "--description",
            "Author the shared OAuth login HTTP contract.",
            "--context",
            "contracts/http/oauth-login.yaml",
        ])
        .assert()
        .success();
    envs.command()
        .args([
            "change",
            "plan",
            "add",
            "add-oauth-tokens",
            "--project",
            "shop-backend",
            "--depends-on",
            "oauth-login-contract",
            "--description",
            "Implement OAuth provider token persistence and refresh endpoints.",
            "--context",
            "contracts/http/oauth-login.yaml",
        ])
        .assert()
        .success();
    envs.command()
        .args([
            "change",
            "plan",
            "add",
            "add-oauth-screens",
            "--project",
            "shop-mobile",
            "--depends-on",
            "oauth-login-contract",
            "--description",
            "Implement login UI and OAuth redirect handling.",
            "--context",
            "contracts/http/oauth-login.yaml",
        ])
        .assert()
        .success();
}

fn assert_registry_and_plan_are_valid(envs: &TestEnv) {
    let registry = envs.command().args(["--format", "json", "registry", "show"]).assert().success();
    let registry = parse_json(&registry.get_output().stdout);
    let projects = registry["registry"]["projects"].as_array().expect("projects");
    assert_eq!(projects.len(), 2);
    assert!(projects.iter().any(|p| p["name"] == "shop-backend"));
    assert!(projects.iter().any(|p| p["name"] == "shop-mobile"));

    envs.command().args(["registry", "validate"]).assert().success();
    envs.command().args(["change", "plan", "validate"]).assert().success();

    let status =
        envs.command().args(["--format", "json", "change", "plan", "status"]).assert().success();
    let status = parse_json(&status.get_output().stdout);
    let entries = status["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|entry| entry["name"] == "oauth-login-contract"));
    assert!(entries.iter().any(|entry| entry["name"] == "add-oauth-tokens"));
    assert!(entries.iter().any(|entry| entry["name"] == "add-oauth-screens"));

    let plan_yaml = fs::read_to_string(envs.path().join("plan.yaml")).expect("read plan.yaml");
    assert!(
        plan_yaml.contains("capability: contracts@v1"),
        "contract slice must target capability"
    );
    assert!(plan_yaml.contains("project: shop-backend"), "backend slice must be routed");
    assert!(plan_yaml.contains("project: shop-mobile"), "mobile slice must be routed");
}

fn sync_workspace(envs: &TestEnv) {
    envs.command().args(["workspace", "sync"]).assert().success();
    for project in ["shop-backend", "shop-mobile"] {
        assert!(
            envs.path().join(".specify/workspace").join(project).is_dir(),
            "{project} workspace slot should be materialized"
        );
    }
}

fn next_entry(envs: &TestEnv) -> Value {
    let assert =
        envs.command().args(["--format", "json", "change", "plan", "next"]).assert().success();
    parse_json(&assert.get_output().stdout)
}

fn transition(envs: &TestEnv, name: &str, target: &str) {
    envs.command().args(["change", "plan", "transition", name, target]).assert().success();
}

fn replay_contract_slice(envs: &TestEnv) {
    let next = next_entry(envs);
    assert_eq!(next["next"], "oauth-login-contract");
    assert_eq!(next["project"], Value::Null);
    assert!(next.get("sources").is_some(), "plan-next must expose sources");
    assert!(next.get("description").is_some(), "plan-next must expose description");

    transition(envs, "oauth-login-contract", "in-progress");
    fs::create_dir_all(envs.path().join(".specify/specs/oauth-login-contract"))
        .expect("mkdir hub specs");
    fs::create_dir_all(envs.path().join(".specify/archive/oauth-login-contract"))
        .expect("mkdir hub archive");
    fs::write(
        envs.path().join(".specify/specs/oauth-login-contract/spec.md"),
        "# OAuth Login Contract\n",
    )
    .expect("write contract spec");
    transition(envs, "oauth-login-contract", "done");
}

fn replay_project_slice(
    envs: &TestEnv, project: &FixtureProject, slice_name: &str, residue_path: &str,
) {
    let next = next_entry(envs);
    assert_eq!(next["next"], slice_name);
    assert_eq!(next["project"], project.name);
    assert!(next.get("sources").is_some(), "plan-next must expose sources");

    let prepared = envs
        .command()
        .args([
            "--format",
            "json",
            "workspace",
            "prepare-branch",
            project.name,
            "--change",
            CHANGE_NAME,
        ])
        .assert()
        .success();
    let prepared = parse_json(&prepared.get_output().stdout);
    assert_eq!(prepared["prepared"], true);
    assert_eq!(prepared["branch"], BRANCH_NAME);
    assert_eq!(prepared["project"], project.name);

    transition(envs, slice_name, "in-progress");
    let slot = envs.path().join(".specify/workspace").join(project.name);
    assert_eq!(git_output(&slot, &["branch", "--show-current"], envs), BRANCH_NAME);

    let spec_dir = slot.join(".specify/specs").join(slice_name);
    let archive_dir = slot.join(".specify/archive").join(slice_name);
    fs::create_dir_all(&spec_dir).expect("mkdir project specs");
    fs::create_dir_all(&archive_dir).expect("mkdir project archive");
    fs::write(spec_dir.join("spec.md"), format!("# {slice_name}\n")).expect("write project spec");
    fs::write(archive_dir.join("proposal.md"), format!("# {slice_name}\n")).expect("write archive");
    run_git(&slot, &["add", ".specify/specs", ".specify/archive"], envs);
    run_git(
        &slot,
        &["commit", "--no-gpg-sign", "-m", &format!("specify: merge {slice_name}")],
        envs,
    );

    let residue = slot.join(residue_path);
    fs::create_dir_all(residue.parent().expect("residue parent")).expect("mkdir residue parent");
    fs::write(&residue, format!("// generated by {slice_name}\n")).expect("write residue");
    run_git(&slot, &["add", residue_path], envs);
    run_git(
        &slot,
        &["commit", "--no-gpg-sign", "-m", &format!("specify: residue {slice_name}")],
        envs,
    );
    assert_eq!(git_output(&slot, &["status", "--porcelain"], envs), "");

    transition(envs, slice_name, "done");

    let log = git_output(&slot, &["log", "--format=%s", "-2"], envs);
    let messages: Vec<&str> = log.lines().collect();
    let expected_residue = format!("specify: residue {slice_name}");
    let expected_merge = format!("specify: merge {slice_name}");
    assert_eq!(messages, [expected_residue.as_str(), expected_merge.as_str()]);
}

fn assert_all_done(envs: &TestEnv) {
    let next = next_entry(envs);
    assert_eq!(next["next"], Value::Null);
    assert_eq!(next["reason"], "all-done");

    let status =
        envs.command().args(["--format", "json", "change", "plan", "status"]).assert().success();
    let status = parse_json(&status.get_output().stdout);
    let entries = status["entries"].as_array().expect("entries");
    assert!(entries.iter().all(|entry| entry["status"] == "done"), "{entries:#?}");
}

fn assert_workspace_ready_for_push(envs: &TestEnv) {
    let status =
        envs.command().args(["--format", "json", "workspace", "status"]).assert().success();
    let status = parse_json(&status.get_output().stdout);
    let slots = status["slots"].as_array().expect("slots");
    assert_eq!(slots.len(), 2);
    for slot in slots {
        assert_eq!(slot["kind"], "git-clone");
        assert_eq!(slot["current-branch"], BRANCH_NAME);
        assert_eq!(slot["dirty"], false);
        assert_eq!(slot["branch-matches-change"], true);
        assert_eq!(slot["project-config-present"], true);
    }
}

fn push_workspace(envs: &TestEnv) {
    let push = envs.command().args(["--format", "json", "workspace", "push"]).assert().success();
    let push = parse_json(&push.get_output().stdout);
    let projects = push["projects"].as_array().expect("projects");
    assert_eq!(projects.len(), 2);
    for project in projects {
        assert_eq!(project["status"], "pushed");
        assert_eq!(project["branch"], BRANCH_NAME);
        assert!(project["pr"].as_u64().is_some(), "PR number missing: {project:#?}");
    }
}

fn finalize_and_assert_archive(envs: &TestEnv) {
    let finalized =
        envs.command().args(["--format", "json", "change", "finalize"]).assert().success();
    let finalized = parse_json(&finalized.get_output().stdout);
    assert_eq!(finalized["initiative"], CHANGE_NAME);
    assert_eq!(finalized["finalized"], true);
    let projects = finalized["projects"].as_array().expect("projects");
    assert_eq!(projects.len(), 2);
    assert!(projects.iter().all(|project| project["status"] == "merged"), "{projects:#?}");
    assert_eq!(finalized["summary"]["merged"], 2);

    assert!(!envs.path().join("plan.yaml").exists(), "root plan.yaml should be archived");
    let archived = finalized["archived"].as_str().expect("archived path");
    assert!(Path::new(archived).is_file(), "archived plan missing: {archived}");
    let archive_dir = envs.path().join(".specify/archive/plans");
    assert!(
        fs::read_dir(archive_dir)
            .expect("read plans archive")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("oauth-login-")),
        "plans archive should contain oauth-login artifacts"
    );
}

fn assert_finalize_is_idempotent(envs: &TestEnv) {
    let second = envs.command().args(["--format", "json", "change", "finalize"]).assert().failure();
    let second = parse_json(&second.get_output().stdout);
    assert_eq!(second["error"], "plan-not-found");
}
