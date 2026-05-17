//! Integration tests for `specify change survey` — staged-candidate
//! ingest. The deterministic core lives in `specify-domain` and is
//! tested exhaustively under `crates/domain/tests/survey_ingest.rs`;
//! the cases here cover the binary surface (clap parse, dispatcher
//! wiring, atomic writes, summary envelope, and error envelope) that
//! cannot be reached from the domain crate.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

mod common;
use common::{Project, parse_stderr, specify};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/change_survey")
}

fn ts_fixture() -> PathBuf {
    fixtures_dir().join("synthetic-ts")
}

/// Build a tiny but valid `surfaces.json` body keyed to the
/// `synthetic-ts` fixture under `tests/fixtures/change_survey/`.
fn happy_candidate(source_key: &str) -> String {
    json!({
        "version": 1,
        "source-key": source_key,
        "language": "typescript",
        "surfaces": [{
            "id": "http-get-users",
            "kind": "http-route",
            "identifier": "GET /users",
            "handler": "src/routes/users.ts:listUsers",
            "touches": ["src/routes/users.ts", "src/users/repository.ts"],
            "declared-at": ["src/routes/users.ts:1"]
        }]
    })
    .to_string()
}

// ── Happy single-source: writes canonical sidecars ─────────────────

#[test]
fn single_source_writes_canonical_sidecars() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let staged = project.root().join("candidate.json");
    fs::write(&staged, happy_candidate("legacy-ts")).unwrap();

    specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            ts_fixture().to_str().unwrap(),
            "--source-key",
            "legacy-ts",
            "--surfaces",
            staged.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.join("surfaces.json").exists(), "surfaces.json must be written");
    assert!(out.join("metadata.json").exists(), "metadata.json must be written");

    let written = fs::read_to_string(out.join("surfaces.json")).unwrap();
    assert!(written.contains("\"source-key\": \"legacy-ts\""), "got: {written}");
}

// ── --validate-only short-circuits writes ──────────────────────────

#[test]
fn validate_only_skips_writes() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let staged = project.root().join("candidate.json");
    fs::write(&staged, happy_candidate("legacy-ts")).unwrap();

    specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            ts_fixture().to_str().unwrap(),
            "--source-key",
            "legacy-ts",
            "--surfaces",
            staged.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--validate-only",
        ])
        .assert()
        .success();

    assert!(!out.join("surfaces.json").exists(), "validate-only must not write surfaces.json");
    assert!(!out.join("metadata.json").exists(), "validate-only must not write metadata.json");
}

// ── staged-input-missing ───────────────────────────────────────────

#[test]
fn staged_input_missing() {
    let project = Project::init();
    let out = project.root().join("survey-out");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            ts_fixture().to_str().unwrap(),
            "--source-key",
            "legacy-ts",
            "--surfaces",
            "/nonexistent/candidate.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "staged-input-missing", "got: {actual}");
}

// ── sources-file-missing ───────────────────────────────────────────

#[test]
fn batch_sources_file_missing() {
    let project = Project::init();
    let out = project.root().join("survey-out");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "--sources",
            "/nonexistent/sources.yaml",
            "--staged",
            "/nonexistent/staged",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "sources-file-missing", "got: {actual}");
}

// ── sources-file-malformed ─────────────────────────────────────────

#[test]
fn batch_sources_file_malformed() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let staged_dir = project.root().join("staged");
    fs::create_dir_all(&staged_dir).unwrap();
    let bad_file = project.root().join("bad-sources.yaml");
    fs::write(&bad_file, "{{not yaml at all").unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "--sources",
            bad_file.to_str().unwrap(),
            "--staged",
            staged_dir.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "sources-file-malformed", "got: {actual}");
}

// ── source-key-mismatch from existing canonical file ───────────────

#[test]
fn source_key_mismatch_from_existing_file() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    fs::create_dir_all(&out).unwrap();
    fs::write(
        out.join("surfaces.json"),
        r#"{"version":1,"source-key":"other","language":"typescript","surfaces":[]}"#,
    )
    .unwrap();
    let staged = project.root().join("candidate.json");
    fs::write(&staged, happy_candidate("not-other")).unwrap();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            ts_fixture().to_str().unwrap(),
            "--source-key",
            "not-other",
            "--surfaces",
            staged.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "source-key-mismatch", "got: {actual}");
}

// ── Mutual exclusion at the clap layer ─────────────────────────────

#[test]
fn mutual_exclusion_single_and_batch() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "change",
            "survey",
            "/some/path",
            "--source-key",
            "k",
            "--surfaces",
            "/some/file.json",
            "--sources",
            "/some/sources.yaml",
            "--staged",
            "/some/staged",
            "--out",
            "/tmp/out",
        ])
        .assert()
        .failure();

    assert_eq!(assert.get_output().status.code(), Some(2), "clap mutual exclusion must exit 2");
}

#[test]
fn neither_form_provided() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "survey", "--out", "/tmp/out"])
        .assert()
        .failure();

    assert_eq!(
        assert.get_output().status.code(),
        Some(2),
        "missing both forms must exit 2 (argument error)"
    );
}
