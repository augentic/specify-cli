//! End-to-end integration tests for the `specify` CLI.
//!
//! Each test stands up a fresh `.specify/` project in a `tempfile::TempDir`,
//! drives the built binary via `assert_cmd`, and compares stdout against a
//! checked-in golden JSON file (under `tests/fixtures/e2e/goldens/`).
//!
//! ## Regenerating goldens
//!
//! Goldens hold the canonical stdout shape after [`strip_substitutions`] has
//! replaced tempdir paths and today's date with deterministic placeholders.
//! When a subcommand's output shape intentionally changes, rerun this file
//! with `REGENERATE_GOLDENS=1` and commit the diff — see
//! [DECISIONS.md](../DECISIONS.md) §"Change J — golden JSON generation".

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

// ---------------------------------------------------------------------------
// Paths + setup helpers
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn e2e_fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/e2e")
}

fn goldens_dir() -> PathBuf {
    e2e_fixtures().join("goldens")
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

const GIT_TEST_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Specify Test"),
    ("GIT_AUTHOR_EMAIL", "specify-test@example.com"),
    ("GIT_COMMITTER_NAME", "Specify Test"),
    ("GIT_COMMITTER_EMAIL", "specify-test@example.com"),
];

fn specify_with_git_identity() -> Command {
    let mut cmd = specify();
    cmd.envs(GIT_TEST_ENV);
    cmd
}

fn run_git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .envs(GIT_TEST_ENV)
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

/// A throwaway `.specify/` project anchored in a temp directory.
struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
    /// Build an empty tempdir and run `specify init` with the in-repo
    /// Omnia capability fixture.
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

    /// Mirror the in-repo `schemas/` tree into the project so
    /// `Capability::resolve("omnia", …)` succeeds.
    fn with_schemas(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join("schemas/omnia"));
        self
    }

    /// Populate the schema cache instead of the local `schemas/` tree so
    /// `Capability::resolve` picks the `CapabilitySource::Cached` branch.
    fn with_cached_schema(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join(".specify/.cache/omnia"));
        self
    }

    /// Copy a fixture subtree into `.specify/slices/my-slice/`.
    fn stage_slice(&self, fixture: &str) -> PathBuf {
        let dst = self.root.join(".specify/slices/my-slice");
        fs::create_dir_all(&dst).expect("mkdir slice");
        copy_dir(&e2e_fixtures().join(fixture), &dst);
        dst
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

// ---------------------------------------------------------------------------
// Substitution / golden comparison
// ---------------------------------------------------------------------------

const TEMPDIR_PLACEHOLDER: &str = "<TEMPDIR>";

/// Substitution rule: literal `from` → placeholder `to`.
struct Sub {
    from: String,
    to: &'static str,
}

impl Sub {
    fn new(from: impl Into<String>, to: &'static str) -> Self {
        Self {
            from: from.into(),
            to,
        }
    }
}

/// Every way the user's tempdir might appear in stdout. macOS canonicalises
/// `/var/folders/...` to `/private/var/folders/...` whenever a subcommand
/// touches the filesystem, so we have to strip both spellings.
///
/// Apply the longest candidate first. On macOS the canonical tempdir
/// path (`/private/var/folders/...`) is a superstring of the raw path
/// (`/var/folders/...`); if we substitute the raw path first, we strip
/// inside the canonical one and leave a stray `/private` prefix in the
/// golden. Sorting by length descending avoids that.
fn tempdir_subs(root: &Path) -> Vec<Sub> {
    let mut subs: Vec<Sub> = Vec::new();
    if let Some(raw) = root.to_str() {
        subs.push(Sub::new(raw.to_string(), TEMPDIR_PLACEHOLDER));
    }
    if let Ok(canonical) = fs::canonicalize(root)
        && let Some(canonical_str) = canonical.to_str()
        && Some(canonical_str) != root.to_str()
    {
        subs.push(Sub::new(canonical_str.to_string(), TEMPDIR_PLACEHOLDER));
    }
    subs.sort_by_key(|s| std::cmp::Reverse(s.from.len()));
    subs
}

/// Recursively walk `value` and apply every substitution to every string.
fn strip_substitutions(value: &mut Value, subs: &[Sub]) {
    match value {
        Value::String(s) => {
            for sub in subs {
                if s.contains(&sub.from) {
                    *s = s.replace(&sub.from, sub.to);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_substitutions(item, subs);
            }
        }
        Value::Object(map) => {
            for (_k, v) in map.iter_mut() {
                strip_substitutions(v, subs);
            }
        }
        _ => {}
    }
}

/// Compare `actual` against the checked-in golden or, when the
/// `REGENERATE_GOLDENS` env var is set, rewrite the golden on disk.
#[allow(clippy::needless_pass_by_value)]
fn assert_golden(name: &str, actual: Value) {
    let golden_path = goldens_dir().join(name);
    let rendered = serde_json::to_string_pretty(&actual).expect("pretty json");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::create_dir_all(goldens_dir()).expect("mkdir goldens");
        fs::write(&golden_path, format!("{rendered}\n")).expect("write golden");
        return;
    }

    let expected_raw = fs::read_to_string(&golden_path).unwrap_or_else(|err| {
        panic!(
            "golden {} missing ({err}); regenerate via REGENERATE_GOLDENS=1 cargo test --test e2e",
            golden_path.display()
        )
    });
    let expected: Value = serde_json::from_str(&expected_raw)
        .unwrap_or_else(|err| panic!("golden {} is not JSON: {err}", golden_path.display()));

    assert_eq!(
        actual,
        expected,
        "stdout diverged from golden {}\n--- actual ---\n{rendered}\n--- expected ---\n{expected_raw}",
        golden_path.display()
    );
}

/// Parse `stdout` as JSON and apply the standard tempdir strip.
fn parse_stdout(stdout: &[u8], root: &Path) -> Value {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    let mut value: Value =
        serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"));
    strip_substitutions(&mut value, &tempdir_subs(root));
    value
}

/// Replace any RFC3339 `YYYY-MM-DDTHH:MM:SS(Z|±HH:MM)` timestamp in JSON
/// strings with the placeholder `<ISO8601>` so goldens stay stable
/// across test runs. Mirrors `change_umbrella.rs::strip_date_stamps`
/// for the timestamp case.
fn strip_iso8601(value: &mut Value) {
    fn visit(re: &regex::Regex, v: &mut Value) {
        match v {
            Value::String(s) if re.is_match(s) => {
                *s = re.replace_all(s, "<ISO8601>").into_owned();
            }
            Value::Array(items) => {
                for item in items {
                    visit(re, item);
                }
            }
            Value::Object(map) => {
                for (_k, v) in map.iter_mut() {
                    visit(re, v);
                }
            }
            _ => {}
        }
    }
    let re = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})")
        .expect("regex compiles");
    visit(&re, value);
}

// ---------------------------------------------------------------------------
// 1. validate — good fixture
// ---------------------------------------------------------------------------

#[test]
fn validate_good_slice_passes() {
    let project = Project::init().with_schemas();
    project.stage_slice("good-slice");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .success();
    assert_eq!(assert.get_output().status.code(), Some(0));

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["passed"], true);
    assert_golden("validate-good.json", actual);
}

// ---------------------------------------------------------------------------
// 2. validate — bad fixture
// ---------------------------------------------------------------------------

#[test]
fn validate_bad_slice_fails_with_exit_two() {
    let project = Project::init().with_schemas();
    project.stage_slice("bad-slice");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "validate on bad fixture must exit 2");

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["passed"], false);
    assert_golden("validate-bad.json", actual);
}

// ---------------------------------------------------------------------------
// 3. merge — two-spec slice
// ---------------------------------------------------------------------------

#[test]
fn merge_two_spec_slice_produces_baselines_and_archive() {
    let project = Project::init().with_schemas();
    project.stage_slice("merge-two-spec-slice");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "merge", "run", "my-slice"])
        .assert()
        .success();

    // Baselines landed under .specify/specs/.
    let login_baseline = project.root().join(".specify/specs/login/spec.md");
    let oauth_baseline = project.root().join(".specify/specs/oauth/spec.md");
    assert!(login_baseline.is_file(), "login baseline must exist");
    assert!(oauth_baseline.is_file(), "oauth baseline must exist");

    // Slice dir moved under archive/<YYYY-MM-DD>-my-slice/.
    let archive_root = project.root().join(".specify/archive");
    let archived: Vec<_> = fs::read_dir(&archive_root)
        .expect("read archive dir")
        .filter_map(std::result::Result::ok)
        .collect();
    assert_eq!(archived.len(), 1, "expected one archived slice");
    let archived_name = archived[0].file_name().to_string_lossy().into_owned();
    assert!(
        archived_name.ends_with("-my-slice"),
        "archive dir name must end with -my-slice, got {archived_name}"
    );
    assert!(
        !project.root().join(".specify/slices/my-slice").exists(),
        "original slice dir should be gone"
    );

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_golden("merge-two-spec.json", actual);
}

#[test]
fn workspace_merge_auto_commit_excludes_generated_residue() {
    let tmp = tempdir().expect("tempdir");
    let project_root = tmp.path().join(".specify/workspace/orders");
    fs::create_dir_all(&project_root).expect("mkdir workspace project");

    specify()
        .current_dir(&project_root)
        .args(["init"])
        .arg(repo_root().join("schemas").join("omnia"))
        .args(["--name", "orders"])
        .assert()
        .success();
    copy_dir(&repo_root().join("schemas/omnia"), &project_root.join("schemas/omnia"));

    run_git(&project_root, &["init"]);
    run_git(&project_root, &["add", "."]);
    run_git(&project_root, &["commit", "-m", "initial project"]);

    let slice_dir = project_root.join(".specify/slices/my-slice");
    copy_dir(&e2e_fixtures().join("merge-two-spec-slice"), &slice_dir);
    fs::create_dir_all(slice_dir.join("contracts/schemas")).expect("mkdir slice contracts");
    fs::write(slice_dir.join("contracts/schemas/generated.yaml"), "openapi: 3.1\n")
        .expect("write generated contract");

    let generated_crate = project_root.join("crates/generated/src/lib.rs");
    fs::create_dir_all(generated_crate.parent().expect("crate parent")).expect("mkdir crate");
    fs::write(&generated_crate, "pub fn generated() {}\n").expect("write generated crate");
    run_git(&project_root, &["add", "crates/generated/src/lib.rs"]);

    specify_with_git_identity()
        .current_dir(&project_root)
        .args(["--format", "json", "slice", "merge", "run", "my-slice"])
        .assert()
        .success();

    let subject = run_git(&project_root, &["log", "-1", "--pretty=%s"]);
    assert_eq!(subject.trim(), "specify: merge my-slice");

    let committed_paths =
        run_git(&project_root, &["show", "--name-only", "--pretty=format:", "HEAD"]);
    let committed_paths: Vec<&str> =
        committed_paths.lines().filter(|line| !line.is_empty()).collect();
    assert!(
        committed_paths.iter().any(|path| path.starts_with(".specify/specs/")),
        "merge commit must include spec baselines, got {committed_paths:?}"
    );
    assert!(
        committed_paths.iter().any(|path| path.starts_with(".specify/archive/")),
        "merge commit must include archived slice, got {committed_paths:?}"
    );
    assert!(
        committed_paths.iter().all(
            |path| path.starts_with(".specify/specs/") || path.starts_with(".specify/archive/")
        ),
        "merge commit must not include generated residue, got {committed_paths:?}"
    );

    let status = run_git(&project_root, &["status", "--porcelain"]);
    assert!(
        status.contains("A  crates/generated/src/lib.rs"),
        "pre-staged generated crate must remain staged for execute residue commit, got:\n{status}"
    );
    assert!(
        status.contains("?? contracts/"),
        "opaque generated contracts must remain uncommitted, got:\n{status}"
    );
    assert!(
        !status
            .lines()
            .any(|line| { line.contains(".specify/specs/") || line.contains(".specify/archive/") }),
        "baseline-owned paths should be clean after merge auto-commit, got:\n{status}"
    );
}

// ---------------------------------------------------------------------------
// 4. task progress
// ---------------------------------------------------------------------------

#[test]
fn task_progress_reports_counts_and_items() {
    let project = Project::init().with_schemas();
    project.stage_slice("good-slice");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "task", "progress", "my-slice"])
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["total"], 5);
    assert_eq!(actual["complete"], 2);
    assert_eq!(actual["pending"], 3);
    assert_golden("task-progress.json", actual);
}

// ---------------------------------------------------------------------------
// 5. task mark — idempotent
// ---------------------------------------------------------------------------

#[test]
fn task_mark_marks_then_is_idempotent() {
    let project = Project::init().with_schemas();
    project.stage_slice("good-slice");
    let tasks_path = project.root().join(".specify/slices/my-slice/tasks.md");

    let before = fs::read_to_string(&tasks_path).expect("read tasks before");
    assert!(before.contains("- [ ] 1.1"), "fixture must start with task 1.1 incomplete");

    // First mark: flips - [ ] -> - [x] and reports idempotent: false.
    let first = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "task", "mark", "my-slice", "1.1"])
        .assert()
        .success();
    let first_value = parse_stdout(&first.get_output().stdout, project.root());
    assert_eq!(first_value["schema-version"], 2);
    assert_eq!(first_value["marked"], "1.1");
    assert_eq!(first_value["idempotent"], false);

    let after_first = fs::read_to_string(&tasks_path).expect("read tasks after 1st mark");
    assert!(after_first.contains("- [x] 1.1"), "tasks.md should now show 1.1 complete");
    assert!(
        !after_first.contains("- [ ] 1.1"),
        "tasks.md should no longer have the incomplete form of 1.1"
    );

    // Second mark: no-op, idempotent: true, file unchanged.
    let second = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "task", "mark", "my-slice", "1.1"])
        .assert()
        .success();
    let second_value = parse_stdout(&second.get_output().stdout, project.root());
    assert_eq!(second_value["idempotent"], true);

    let after_second = fs::read_to_string(&tasks_path).expect("read tasks after 2nd mark");
    assert_eq!(after_first, after_second, "second mark must leave tasks.md byte-identical");

    assert_golden("task-mark.json", second_value);
}

// ---------------------------------------------------------------------------
// 6. capability resolve — local
// ---------------------------------------------------------------------------

#[test]
fn capability_resolve_local_returns_local_source() {
    let project = Project::init().with_schemas();
    fs::remove_dir_all(project.root().join(".specify/.cache/omnia"))
        .expect("remove cached capability");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "resolve", "omnia"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["capability-value"], "omnia");
    assert_eq!(actual["source"], "local");
    let resolved = actual["resolved-path"].as_str().expect("resolved-path str");
    assert!(
        resolved.ends_with("schemas/omnia"),
        "resolved_path {resolved} must end with schemas/omnia"
    );
}

// ---------------------------------------------------------------------------
// 7. capability resolve — cached
// ---------------------------------------------------------------------------

#[test]
fn capability_resolve_cached_returns_cached_source() {
    // `init` + cache-only layout (no `schemas/omnia` under the tempdir).
    let project = Project::init().with_cached_schema();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "capability", "resolve", "omnia"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["source"], "cached");
    let resolved = actual["resolved-path"].as_str().expect("resolved-path str");
    assert!(
        resolved.ends_with(".specify/.cache/omnia"),
        "resolved_path {resolved} must end with .specify/.cache/omnia"
    );
}

// ---------------------------------------------------------------------------
// 8. slice outcome — round-trip through `outcome set` + `outcome show`
// ---------------------------------------------------------------------------

/// End-to-end round-trip for the `slice outcome` read verb added in
/// RFC-2 §1.1 (renamed from `change outcome` in RFC-13 chunk 3.2):
/// stamp an outcome with `slice outcome set`, read it back with
/// `slice outcome show --format json`, and assert the full JSON
/// shape. Also covers the unstamped case where `outcome` must be `null`.
#[test]
fn phase_outcome_round_trip_via_slice_outcome_verb() {
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
    assert_eq!(assert.get_output().status.code(), Some(0));

    let mut actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["name"], "foo");
    let outcome = &actual["outcome"];
    assert_eq!(outcome["phase"], "build");
    assert_eq!(outcome["outcome"], "success");
    assert_eq!(outcome["summary"], "5/5 tasks");
    assert_eq!(outcome["context"], "trailing newline");
    let at = outcome["at"].as_str().expect("at is a string");
    let at_re =
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$").expect("regex compiles");
    assert!(
        at_re.is_match(at),
        "at must match ^\\d{{4}}-\\d{{2}}-\\d{{2}}T\\d{{2}}:\\d{{2}}:\\d{{2}}Z$, got {at}"
    );

    strip_iso8601(&mut actual);
    assert_golden("slice-outcome.json", actual);

    let unstamped =
        specify().current_dir(project.root()).args(["slice", "create", "bar"]).assert().success();
    assert_eq!(unstamped.get_output().status.code(), Some(0));

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "bar"])
        .assert()
        .success();
    assert_eq!(assert.get_output().status.code(), Some(0));

    let value = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["name"], "bar");
    assert!(
        value["outcome"].is_null(),
        "unstamped slice must emit outcome == null, got: {}",
        value["outcome"]
    );
}
