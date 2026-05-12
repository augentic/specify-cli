//! Helpers shared across the binary's integration tests.
//!
//! Each test file `mod common;` to pull these in (cargo's "include
//! shared module" idiom for `tests/`). Some test files use only a
//! subset, so the items are tagged `#[allow(dead_code)]` to keep
//! lints quiet.

#![expect(
    unreachable_pub,
    reason = "test helpers shared across integration test binaries; each `tests/*.rs` is its own crate so `pub(crate)` is wrong here"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use serde_json::Value;
use specify_error::Result;
use tempfile::{TempDir, tempdir};

/// Panic with a descriptive message when a handler returned an error.
///
/// Mirrors the inline `assert_ok` previously colocated with `specify
/// registry`'s unit tests. Hoisted here so future integration tests can
/// share the same `Result<()>`-shaped success check without re-inventing
/// the wrapper.
#[allow(dead_code)]
#[track_caller]
pub fn assert_ok(result: Result<()>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// Path to the workspace root for the `specify` crate (where the
/// integration tests live).
#[allow(dead_code)]
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Convenience pointer to the in-repo Omnia capability fixture used as
/// the canonical positional argument for `specify init`.
#[allow(dead_code)]
pub fn omnia_schema_dir() -> PathBuf {
    repo_root().join("schemas").join("omnia")
}

/// Build a fresh `assert_cmd::Command` for the locally-built `specify`
/// binary.
#[allow(dead_code)]
pub fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

/// Deterministic git author/committer identity for tests that exercise
/// real `git commit` invocations.
#[allow(dead_code)]
pub const GIT_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Specify Test"),
    ("GIT_AUTHOR_EMAIL", "specify-test@example.com"),
    ("GIT_COMMITTER_NAME", "Specify Test"),
    ("GIT_COMMITTER_EMAIL", "specify-test@example.com"),
];

/// Run `git` in `root` with [`GIT_ENV`] applied, asserting success
/// and returning captured stdout.
///
/// # Panics
///
/// Panics if git fails to start or exits non-zero.
#[allow(dead_code)]
pub fn run_git(root: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
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

/// Parse a captured stdout buffer as JSON, panicking on UTF-8 or parse
/// errors with the offending text included for debugging.
///
/// # Panics
///
/// Panics if `stdout` is not UTF-8 or not valid JSON.
#[allow(dead_code)]
pub fn parse_json(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"))
}

/// Recursively copy `src` into `dst`, creating directories as needed.
///
/// # Panics
///
/// Panics if a fixture directory cannot be read or copied into the test
/// workspace.
#[allow(dead_code)]
pub fn copy_dir(src: &Path, dst: &Path) {
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

/// Scaffold an empty hub-mode project in `tmp` via `specify init --hub`.
///
/// # Panics
///
/// Panics if the `specify init` invocation does not exit 0.
#[allow(dead_code)]
pub fn init_hub(tmp: &TempDir, name: &str) {
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .args(["--name", name, "--hub"])
        .assert()
        .success();
}

/// Placeholder substituted in for the test's tempdir path before
/// comparing stdout against a checked-in golden.
#[allow(dead_code)]
pub const TEMPDIR_PLACEHOLDER: &str = "<TEMPDIR>";

/// String-replacement rule applied to every JSON string before golden
/// comparison.
#[allow(dead_code)]
pub struct Sub {
    pub from: String,
    pub to: &'static str,
}

impl Sub {
    #[allow(dead_code)]
    pub fn new(from: impl Into<String>, to: &'static str) -> Self {
        Self {
            from: from.into(),
            to,
        }
    }
}

/// Substitutions covering every way the tempdir at `root` might appear
/// in stdout.
///
/// macOS canonicalises `/var/folders/...` to `/private/var/folders/...`
/// whenever a subcommand touches the filesystem, so both spellings are
/// stripped. Sorting by length descending guarantees the longer
/// canonical path is replaced first; otherwise the shorter raw path
/// would match inside the canonical one and leave a stray `/private`
/// prefix in the golden.
#[allow(dead_code)]
pub fn tempdir_subs(root: &Path) -> Vec<Sub> {
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

/// Walk `value` recursively and replace every occurrence of
/// `sub.from` with `sub.to` in any contained string.
#[allow(dead_code)]
pub fn strip_substitutions(value: &mut Value, subs: &[Sub]) {
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

/// Parse `stdout` as JSON and apply [`tempdir_subs`] for `root`.
///
/// # Panics
///
/// Panics if `stdout` is not UTF-8 or not valid JSON.
#[allow(dead_code)]
pub fn parse_stdout(stdout: &[u8], root: &Path) -> Value {
    parse_json_stream("stdout", stdout, root)
}

/// Mirror of [`parse_stdout`] for the stderr channel. Used by failure
/// tests since R4 routes every error envelope (JSON or text) through
/// `Stream::Stderr`.
///
/// # Panics
///
/// Panics if `stderr` is not UTF-8 or not valid JSON.
#[allow(dead_code)]
pub fn parse_stderr(stderr: &[u8], root: &Path) -> Value {
    parse_json_stream("stderr", stderr, root)
}

fn parse_json_stream(label: &str, bytes: &[u8], root: &Path) -> Value {
    let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("utf8 {label}"));
    let mut value: Value = serde_json::from_str(text)
        .unwrap_or_else(|err| panic!("{label} not JSON ({err}):\n{text}"));
    strip_substitutions(&mut value, &tempdir_subs(root));
    value
}

/// A throwaway `.specify/` project rooted in a tempdir, scaffolded by
/// running `specify init` with the in-repo Omnia capability fixture.
///
/// Hoisted from the per-test-file `struct Project` harnesses
/// (`tests/slice.rs`, `tests/slice_merge.rs`, `tests/e2e.rs`,
/// `tests/capability.rs`, `tests/change_plan_orchestrate.rs`) so the same
/// `Project::init()` / `.with_schemas()` / `.stage_slice()` shape works
/// across every integration suite. Each test binary uses a different
/// subset, hence the `#[allow(dead_code)]` on every public item.
pub struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
    /// Build a fresh tempdir and run `specify init <repo>/schemas/omnia`
    /// with a default `--name`. The resulting project sits at the
    /// tempdir root.
    #[allow(dead_code)]
    pub fn init() -> Self {
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

    /// Initialise a project backed by a local fixture capability dir.
    /// The fixture is mirrored into `<tmp>/schemas/<name>/` so that
    /// subsequent `specify` invocations resolve it via the usual
    /// `schemas/<name>/` probe.
    #[allow(dead_code)]
    pub fn init_from_fixture(name: &str, fixture_dir: &Path) -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        copy_dir(fixture_dir, &root.join("schemas").join(name));
        specify()
            .current_dir(&root)
            .args(["init"])
            .arg(root.join("schemas").join(name))
            .args(["--name", "test-proj"])
            .assert()
            .success();
        Self { _tmp: tmp, root }
    }

    /// Mirror the in-repo `schemas/omnia` tree into the project so any
    /// subcommand that loads a `PipelineView` can resolve the schema
    /// from the project's own `schemas/` dir.
    #[allow(dead_code)]
    #[must_use]
    pub fn with_schemas(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join("schemas/omnia"));
        self
    }

    /// Populate the schema cache instead of the local `schemas/` tree so
    /// `Capability::resolve` picks the `CapabilitySource::Cached` branch.
    #[allow(dead_code)]
    #[must_use]
    pub fn with_cached_schema(self) -> Self {
        copy_dir(&repo_root().join("schemas/omnia"), &self.root.join(".specify/.cache/omnia"));
        self
    }

    /// Copy a fixture subtree under `tests/fixtures/e2e/<fixture>` into
    /// `.specify/slices/my-slice/` and return the slice directory path.
    #[allow(dead_code)]
    pub fn stage_slice(&self, fixture: &str) -> PathBuf {
        let dst = self.root.join(".specify/slices/my-slice");
        fs::create_dir_all(&dst).expect("mkdir slice");
        copy_dir(&repo_root().join("tests/fixtures/e2e").join(fixture), &dst);
        dst
    }

    /// Path to the project root (the tempdir).
    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to `.specify/slices/` under the project root.
    #[allow(dead_code)]
    pub fn slices_dir(&self) -> PathBuf {
        self.root.join(".specify/slices")
    }

    /// Path to `.specify/specs/` under the project root.
    #[allow(dead_code)]
    pub fn specs_dir(&self) -> PathBuf {
        self.root.join(".specify/specs")
    }

    /// Path to the umbrella `plan.yaml` at the repo root.
    #[allow(dead_code)]
    pub fn plan_path(&self) -> PathBuf {
        self.root.join("plan.yaml")
    }

    /// Seed `plan.yaml` (at the project root) with arbitrary YAML. Used
    /// by the change-umbrella tests to drive the file directly without
    /// going through the `plan create` verb.
    #[allow(dead_code)]
    pub fn seed_plan(&self, yaml: &str) {
        fs::write(self.plan_path(), yaml).expect("write plan.yaml");
    }
}

/// Compare `actual` against the golden at `dir/name`, or rewrite that
/// golden when the `REGENERATE_GOLDENS` env var is set.
///
/// # Panics
///
/// Panics if the golden cannot be read, is not JSON, or differs from
/// `actual`.
#[allow(dead_code)]
#[allow(clippy::needless_pass_by_value)]
pub fn assert_golden_at(dir: &Path, name: &str, actual: Value) {
    let golden_path = dir.join(name);
    let rendered = serde_json::to_string_pretty(&actual).expect("pretty json");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::create_dir_all(dir).expect("mkdir golden dir");
        fs::write(&golden_path, format!("{rendered}\n")).expect("write golden");
        return;
    }

    let expected_raw = fs::read_to_string(&golden_path).unwrap_or_else(|err| {
        panic!(
            "golden {} missing ({err}); regenerate via REGENERATE_GOLDENS=1 cargo test",
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
