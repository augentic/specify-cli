//! Integration tests for `specify change survey` — mechanical source
//! scanner. Covers every RFC-listed exit discriminant, byte-stable
//! golden output, mutual exclusion, per-row independence in batch mode,
//! and source-key mismatch guard.

use std::fs;
use std::path::PathBuf;

mod common;
use common::{Project, parse_stderr, specify};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/change_survey")
}

fn ts_fixture() -> PathBuf {
    fixtures_dir().join("synthetic-ts")
}

// ── Single-source: no-detectors (empty registry) ────────────────────

#[test]
fn single_source_no_detectors() {
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
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "no-detectors", "expected no-detectors, got: {actual}");

    assert!(!out.join("surfaces.json").exists(), "no surfaces.json should be written on failure");
    assert!(!out.join("metadata.json").exists(), "no metadata.json should be written on failure");
}

// ── source-path-missing ─────────────────────────────────────────────

#[test]
fn single_source_path_missing() {
    let project = Project::init();
    let out = project.root().join("survey-out");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "/nonexistent/path/to/source",
            "--source-key",
            "missing",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(
        actual["exit-code"], 2,
        "source-path-missing maps to EXIT_VALIDATION_FAILED (2): {actual}"
    );
}

// ── source-path-not-readable ────────────────────────────────────────

#[test]
fn single_source_path_not_readable() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let not_dir = project.root().join("not-a-dir");
    fs::write(&not_dir, "i am a file not a directory").expect("create file");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            not_dir.to_str().unwrap(),
            "--source-key",
            "bad",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(
        actual["error"], "source-path-not-readable",
        "expected source-path-not-readable, got: {actual}"
    );
}

// ── sources-file-missing ────────────────────────────────────────────

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
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(
        actual["exit-code"], 2,
        "sources-file-missing maps to EXIT_VALIDATION_FAILED: {actual}"
    );
    let msg = actual["message"].as_str().unwrap_or_default();
    assert!(msg.contains("not found"), "message should say not found, got: {msg}");
}

// ── sources-file-malformed ──────────────────────────────────────────

#[test]
fn batch_sources_file_malformed() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let bad_file = project.root().join("bad-sources.yaml");
    fs::write(&bad_file, "{{not yaml at all").expect("write bad sources");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "--sources",
            bad_file.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(
        actual["exit-code"], 2,
        "sources-file-malformed maps to EXIT_VALIDATION_FAILED: {actual}"
    );
}

#[test]
fn batch_sources_file_duplicate_key() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let dup_file = project.root().join("dup-sources.yaml");
    fs::write(
        &dup_file,
        "\
version: 1
sources:
  - key: same
    path: ./a
  - key: same
    path: ./b
",
    )
    .expect("write dup sources");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "--sources",
            dup_file.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    let msg = actual["message"].as_str().unwrap_or_default();
    assert!(msg.contains("duplicate key"), "expected duplicate key error, got: {msg}");
}

// ── Mutual exclusion ────────────────────────────────────────────────

#[test]
fn mutual_exclusion_source_path_and_sources() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "change",
            "survey",
            "/some/path",
            "--source-key",
            "k",
            "--sources",
            "/some/file.yaml",
            "--out",
            "/tmp/out",
        ])
        .assert()
        .failure();

    assert_eq!(assert.get_output().status.code(), Some(2), "clap mutual exclusion must exit 2");
}

#[test]
fn neither_source_path_nor_sources() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "survey", "--out", "/tmp/out"])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["exit-code"], 2, "missing both forms must exit 2: {actual}");
}

// ── source-key-mismatch ─────────────────────────────────────────────

#[test]
fn source_key_mismatch() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    fs::create_dir_all(&out).expect("create out dir");
    fs::write(
        out.join("surfaces.json"),
        r#"{"version":1,"source-key":"other","language":"typescript","surfaces":[]}"#,
    )
    .expect("seed mismatched surfaces.json");

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
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(
        actual["error"], "source-key-mismatch",
        "expected source-key-mismatch, got: {actual}"
    );
}

// ── Batch: per-row independence ─────────────────────────────────────

#[test]
fn batch_per_row_independence() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let sources_file = project.root().join("sources.yaml");

    fs::write(
        &sources_file,
        format!(
            "\
version: 1
sources:
  - key: good
    path: {}
  - key: bad
    path: /nonexistent/source/path
",
            ts_fixture().display()
        ),
    )
    .expect("write sources");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "--sources",
            sources_file.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert!(
        !actual["error"].as_str().unwrap_or_default().is_empty(),
        "batch with failures should report an error: {actual}"
    );

    // Row "good" still fails with no-detectors (empty registry), so its
    // files should NOT be written either. Both rows fail independently.
    assert!(
        !out.join("good/surfaces.json").exists(),
        "failed row's surfaces.json should not exist"
    );
    assert!(
        !out.join("bad/surfaces.json").exists(),
        "non-existent source row's files should not exist"
    );
}

// ── Batch: valid sources file with path-missing row ─────────────────

#[test]
fn batch_source_path_missing_in_row() {
    let project = Project::init();
    let out = project.root().join("survey-out");
    let sources_file = project.root().join("sources.yaml");

    fs::write(
        &sources_file,
        "\
version: 1
sources:
  - key: missing
    path: /nonexistent/path
",
    )
    .expect("write sources");

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "change",
            "survey",
            "--sources",
            sources_file.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    let msg = actual["message"].as_str().unwrap_or_default();
    assert!(msg.contains("does not exist"), "batch row should report source-path-missing: {msg}");
}

// ── Byte-stable golden ─────────────────────────────────────────────
// With an empty detector registry, single-source always exits
// `no-detectors`, so golden tests on the *error* envelope shape are
// the most stable contract we can assert without mocking detectors
// at the binary integration level.

#[test]
fn no_detectors_error_shape_is_stable() {
    let project = Project::init();
    let out = project.root().join("survey-out");

    let run = |_label: &str| -> Vec<u8> {
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
                "--out",
                out.to_str().unwrap(),
            ])
            .assert()
            .failure();
        assert.get_output().stderr.clone()
    };

    let first = run("first");
    let second = run("second");
    assert_eq!(first, second, "error output must be byte-stable across runs");
}
