//! Integration tests for `specify spec preview` and `specify spec conflict-check`.
//!
//! These are the two no-write counterparts to `specify merge` used by the
//! merge-skill rewrite: `preview` computes the operation list without
//! touching disk; `conflict-check` flags `type: modified` baselines that
//! have drifted since `defined_at`.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn parse_json(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"))
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

struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
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
        copy_dir(&repo_root().join("schemas/omnia"), &root.join("schemas/omnia"));

        Project { _tmp: tmp, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn stage_change(&self, fixture: &str) -> PathBuf {
        let dst = self.root.join(".specify/changes/my-change");
        fs::create_dir_all(&dst).expect("mkdir change");
        copy_dir(&repo_root().join("tests/fixtures/e2e").join(fixture), &dst);
        dst
    }
}

// ---------------------------------------------------------------------------
// spec preview
// ---------------------------------------------------------------------------

#[test]
fn spec_preview_reports_operations_without_writing() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "preview"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 2);

    let specs = value["specs"].as_array().expect("specs array");
    // The two-spec fixture has both `login` and `oauth`; each uses a
    // `## ADDED Requirements` section with one REQ-001 block, so each
    // preview entry should carry exactly one `added` operation (the
    // `created_baseline` op only fires for verbatim copies without
    // delta headers — see merge-two-spec.json golden).
    let names: Vec<&str> = specs.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["login", "oauth"]);
    for spec in specs {
        let ops = spec["operations"].as_array().unwrap();
        assert_eq!(ops.len(), 1, "expected one op per spec, got {ops:?}");
        assert_eq!(ops[0]["kind"], "added");
        assert_eq!(ops[0]["id"], "REQ-001");
        assert!(spec["baseline-path"].is_string());
    }

    // No filesystem mutation: no archive, change dir still in place,
    // baselines under .specify/specs/ untouched.
    assert!(change_dir.is_dir(), "preview must not archive the change");
    let archive = project.root().join(".specify/archive");
    assert!(
        !archive.exists() || fs::read_dir(&archive).unwrap().next().is_none(),
        "preview must not create archive entries",
    );
    assert!(
        !project.root().join(".specify/specs/login/spec.md").exists(),
        "preview must not write baselines",
    );
    assert!(
        !project.root().join(".specify/specs/oauth/spec.md").exists(),
        "preview must not write baselines",
    );
}

#[test]
fn spec_preview_does_not_require_complete_status() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");
    // Downgrade status to `building` — `specify merge` refuses this but
    // `specify spec preview` must accept it.
    let metadata_path = change_dir.join(".metadata.yaml");
    let original = fs::read_to_string(&metadata_path).unwrap();
    let downgraded = original.replace("status: complete", "status: building");
    fs::write(&metadata_path, downgraded).unwrap();

    specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "preview"])
        .arg(&change_dir)
        .assert()
        .success();
}

#[test]
fn spec_preview_emits_readable_text_output() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["spec", "preview"])
        .arg(&change_dir)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("login:"));
    assert!(stdout.contains("oauth:"));
    assert!(
        stdout.contains("ADDING: REQ-001"),
        "expected ADDING line in text output, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// spec conflict-check
// ---------------------------------------------------------------------------

#[test]
fn conflict_check_reports_no_conflicts_when_no_modified_entries() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let conflicts = value["conflicts"].as_array().unwrap();
    assert!(conflicts.is_empty(), "fixture has only `new` entries, got {conflicts:?}");
}

#[test]
fn conflict_check_flags_modified_baseline_newer_than_defined_at() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    // Seed a baseline file under .specify/specs/login/spec.md then rewrite
    // the change's metadata to mark `login` as `modified` with a historic
    // defined_at. touching the baseline afterwards puts its mtime in the
    // future relative to defined_at, producing a conflict.
    let baseline = project.root().join(".specify/specs/login/spec.md");
    fs::create_dir_all(baseline.parent().unwrap()).unwrap();
    fs::write(&baseline, "# Login baseline\n").unwrap();

    let metadata_path = change_dir.join(".metadata.yaml");
    fs::write(
        &metadata_path,
        "schema: omnia\nstatus: complete\ndefined-at: \"2020-01-01T00:00:00Z\"\ntouched-specs:\n  - name: login\n    type: modified\n",
    )
    .unwrap();

    // Nudge mtime forward — on macOS the setup above already yields a
    // post-2020 mtime, but be explicit so the test is insensitive to
    // clock skew or filesystem resolution.
    sleep(Duration::from_millis(10));
    fs::write(&baseline, "# Login baseline (touched)\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let conflicts = value["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "expected one conflict, got {conflicts:?}");
    assert_eq!(conflicts[0]["capability"], "login");
    assert_eq!(conflicts[0]["defined-at"], "2020-01-01T00:00:00Z");
    assert!(conflicts[0]["baseline-modified-at"].is_string());
}

#[test]
fn conflict_check_no_contract_drift_when_baseline_is_older() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    // Set defined_at to the far future so nothing is "newer".
    let metadata_path = change_dir.join(".metadata.yaml");
    fs::write(
        &metadata_path,
        "schema: omnia\nstatus: complete\ndefined-at: \"2099-01-01T00:00:00Z\"\ntouched-specs:\n  - name: login\n    type: new\n",
    )
    .unwrap();

    // Seed a baseline contract file (its mtime will be well before 2099).
    let baseline_contract = project.root().join(".specify/contracts/schemas/test.yaml");
    fs::create_dir_all(baseline_contract.parent().unwrap()).unwrap();
    fs::write(&baseline_contract, "type: object\n").unwrap();

    // Seed the corresponding change contract so the drift walker visits it.
    let change_contract = change_dir.join("contracts/schemas/test.yaml");
    fs::create_dir_all(change_contract.parent().unwrap()).unwrap();
    fs::write(&change_contract, "type: object\nproperties: {}\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let conflicts = value["conflicts"].as_array().unwrap();
    assert!(
        conflicts.is_empty(),
        "baseline is older than defined_at, expected no conflicts, got {conflicts:?}"
    );
}

#[test]
fn conflict_check_detects_contract_drift_when_baseline_is_newer() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    // defined_at in the deep past — any real file mtime will be newer.
    let metadata_path = change_dir.join(".metadata.yaml");
    fs::write(
        &metadata_path,
        "schema: omnia\nstatus: complete\ndefined-at: \"2020-01-01T00:00:00Z\"\ntouched-specs:\n  - name: login\n    type: new\n",
    )
    .unwrap();

    let baseline_contract = project.root().join(".specify/contracts/schemas/test.yaml");
    fs::create_dir_all(baseline_contract.parent().unwrap()).unwrap();
    fs::write(&baseline_contract, "type: object\n").unwrap();

    // Nudge mtime forward.
    sleep(Duration::from_millis(10));
    fs::write(&baseline_contract, "type: object # touched\n").unwrap();

    let change_contract = change_dir.join("contracts/schemas/test.yaml");
    fs::create_dir_all(change_contract.parent().unwrap()).unwrap();
    fs::write(&change_contract, "type: object\nproperties: {}\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let conflicts = value["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "expected one contract conflict, got {conflicts:?}");
    assert_eq!(conflicts[0]["capability"], "contracts/schemas/test.yaml");
    assert_eq!(conflicts[0]["defined-at"], "2020-01-01T00:00:00Z");
}

#[test]
fn conflict_check_no_drift_for_new_contract_files() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    let metadata_path = change_dir.join(".metadata.yaml");
    fs::write(
        &metadata_path,
        "schema: omnia\nstatus: complete\ndefined-at: \"2020-01-01T00:00:00Z\"\ntouched-specs:\n  - name: login\n    type: new\n",
    )
    .unwrap();

    // Change has a contract file, but no corresponding baseline exists.
    let change_contract = change_dir.join("contracts/schemas/new.yaml");
    fs::create_dir_all(change_contract.parent().unwrap()).unwrap();
    fs::write(&change_contract, "type: object\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let conflicts = value["conflicts"].as_array().unwrap();
    assert!(
        conflicts.is_empty(),
        "new contract files (not in baseline) should not produce conflicts, got {conflicts:?}"
    );
}

#[test]
fn conflict_check_no_drift_when_change_has_no_contracts_dir() {
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");

    let metadata_path = change_dir.join(".metadata.yaml");
    fs::write(
        &metadata_path,
        "schema: omnia\nstatus: complete\ndefined-at: \"2020-01-01T00:00:00Z\"\ntouched-specs:\n  - name: login\n    type: new\n",
    )
    .unwrap();

    // Seed a baseline contract but do NOT create contracts/ in the change.
    let baseline_contract = project.root().join(".specify/contracts/schemas/test.yaml");
    fs::create_dir_all(baseline_contract.parent().unwrap()).unwrap();
    fs::write(&baseline_contract, "type: object\n").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let conflicts = value["conflicts"].as_array().unwrap();
    assert!(
        conflicts.is_empty(),
        "no contracts/ in the change means no contract drift, got {conflicts:?}"
    );
}

#[test]
fn conflict_check_ignores_new_entries_even_with_existing_baseline() {
    // `type: new` baselines are "we're creating this capability" — even
    // if a file already exists at the baseline path, it is not a drift
    // conflict in the mtime-vs-defined_at sense, just a different kind
    // of integrity issue the caller should handle separately.
    let project = Project::init();
    let change_dir = project.stage_change("merge-two-spec-change");
    let baseline = project.root().join(".specify/specs/login/spec.md");
    fs::create_dir_all(baseline.parent().unwrap()).unwrap();
    fs::write(&baseline, "# Login baseline\n").unwrap();

    // touched_specs keeps the fixture's `new` classification; no
    // `defined_at` means conflict_check returns empty regardless.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "spec", "conflict-check"])
        .arg(&change_dir)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["conflicts"].as_array().unwrap().is_empty());
}
