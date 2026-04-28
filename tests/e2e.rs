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
    /// Build an empty tempdir and run `specify init omnia` with `--schema-dir`
    /// pointed at the repo root.
    fn init() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        specify()
            .current_dir(&root)
            .args(["init", "omnia", "--schema-dir"])
            .arg(repo_root())
            .args(["--name", "test-proj"])
            .assert()
            .success();
        Self { _tmp: tmp, root }
    }

    /// Mirror the in-repo `schemas/` tree into the project so
    /// `Schema::resolve("omnia", …)` succeeds.
    fn with_schemas(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join("schemas/omnia"));
        self
    }

    /// Populate the schema cache instead of the local `schemas/` tree so
    /// `Schema::resolve` picks the `SchemaSource::Cached` branch.
    fn with_cached_schema(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join(".specify/.cache/omnia"));
        self
    }

    /// Copy a fixture subtree into `.specify/changes/my-change/`.
    fn stage_change(&self, fixture: &str) -> PathBuf {
        let dst = self.root.join(".specify/changes/my-change");
        fs::create_dir_all(&dst).expect("mkdir change");
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
/// across test runs. Mirrors `initiative.rs::strip_date_stamps` for
/// the timestamp case.
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
fn validate_good_change_passes() {
    let project = Project::init().with_schemas();
    project.stage_change("good-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "validate", "my-change"])
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
fn validate_bad_change_fails_with_exit_two() {
    let project = Project::init().with_schemas();
    project.stage_change("bad-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "validate", "my-change"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "validate on bad fixture must exit 2");

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["passed"], false);
    assert_golden("validate-bad.json", actual);
}

// ---------------------------------------------------------------------------
// 3. merge — two-spec change
// ---------------------------------------------------------------------------

#[test]
fn merge_two_spec_change_produces_baselines_and_archive() {
    let project = Project::init().with_schemas();
    project.stage_change("merge-two-spec-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "merge", "run", "my-change"])
        .assert()
        .success();

    // Baselines landed under .specify/specs/.
    let login_baseline = project.root().join(".specify/specs/login/spec.md");
    let oauth_baseline = project.root().join(".specify/specs/oauth/spec.md");
    assert!(login_baseline.is_file(), "login baseline must exist");
    assert!(oauth_baseline.is_file(), "oauth baseline must exist");

    // Change dir moved under archive/<YYYY-MM-DD>-my-change/.
    let archive_root = project.root().join(".specify/archive");
    let archived: Vec<_> = fs::read_dir(&archive_root)
        .expect("read archive dir")
        .filter_map(std::result::Result::ok)
        .collect();
    assert_eq!(archived.len(), 1, "expected one archived change");
    let archived_name = archived[0].file_name().to_string_lossy().into_owned();
    assert!(
        archived_name.ends_with("-my-change"),
        "archive dir name must end with -my-change, got {archived_name}"
    );
    assert!(
        !project.root().join(".specify/changes/my-change").exists(),
        "original change dir should be gone"
    );

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_golden("merge-two-spec.json", actual);
}

// ---------------------------------------------------------------------------
// 4. task progress
// ---------------------------------------------------------------------------

#[test]
fn task_progress_reports_counts_and_items() {
    let project = Project::init().with_schemas();
    project.stage_change("good-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "task", "progress", "my-change"])
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
    project.stage_change("good-change");
    let tasks_path = project.root().join(".specify/changes/my-change/tasks.md");

    let before = fs::read_to_string(&tasks_path).expect("read tasks before");
    assert!(before.contains("- [ ] 1.1"), "fixture must start with task 1.1 incomplete");

    // First mark: flips - [ ] -> - [x] and reports idempotent: false.
    let first = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "task", "mark", "my-change", "1.1"])
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
        .args(["--format", "json", "change", "task", "mark", "my-change", "1.1"])
        .assert()
        .success();
    let second_value = parse_stdout(&second.get_output().stdout, project.root());
    assert_eq!(second_value["idempotent"], true);

    let after_second = fs::read_to_string(&tasks_path).expect("read tasks after 2nd mark");
    assert_eq!(after_first, after_second, "second mark must leave tasks.md byte-identical");

    assert_golden("task-mark.json", second_value);
}

// ---------------------------------------------------------------------------
// 6. schema resolve — local
// ---------------------------------------------------------------------------

#[test]
fn schema_resolve_local_returns_local_source() {
    let project = Project::init().with_schemas();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "resolve", "omnia"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["schema-version"], 2);
    assert_eq!(actual["schema-value"], "omnia");
    assert_eq!(actual["source"], "local");
    let resolved = actual["resolved-path"].as_str().expect("resolved-path str");
    assert!(
        resolved.ends_with("schemas/omnia"),
        "resolved_path {resolved} must end with schemas/omnia"
    );
}

// ---------------------------------------------------------------------------
// 7. schema resolve — cached
// ---------------------------------------------------------------------------

#[test]
fn schema_resolve_cached_returns_cached_source() {
    // `init` + cache-only layout (no `schemas/omnia` under the tempdir).
    let project = Project::init().with_cached_schema();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "schema", "resolve", "omnia"])
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
// 8. change outcome — round-trip through `outcome set` + `outcome show`
// ---------------------------------------------------------------------------

/// End-to-end round-trip for the `change outcome` read verb added in
/// RFC-2 §1.1: stamp an outcome with `change outcome set`, read it back
/// with `change outcome show --format json`, and assert the full JSON
/// shape. Also covers the unstamped case where `outcome` must be `null`.
#[test]
fn phase_outcome_round_trip_via_change_outcome_verb() {
    let project = Project::init();

    specify().current_dir(project.root()).args(["change", "create", "foo"]).assert().success();
    specify()
        .current_dir(project.root())
        .args([
            "change",
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
        .args(["--format", "json", "change", "outcome", "show", "foo"])
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
    assert_golden("change-outcome.json", actual);

    let unstamped =
        specify().current_dir(project.root()).args(["change", "create", "bar"]).assert().success();
    assert_eq!(unstamped.get_output().status.code(), Some(0));

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "outcome", "show", "bar"])
        .assert()
        .success();
    assert_eq!(assert.get_output().status.code(), Some(0));

    let value = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["name"], "bar");
    assert!(
        value["outcome"].is_null(),
        "unstamped change must emit outcome == null, got: {}",
        value["outcome"]
    );
}
