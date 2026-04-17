//! Integration tests for the `specify` CLI binary.
//!
//! Each test spawns the built binary via `assert_cmd::Command::cargo_bin`
//! from a fresh `tempfile::TempDir`, so stdout/stderr and filesystem side
//! effects are observed exactly as a user would experience them.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use tempfile::tempdir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

#[test]
fn help_exits_zero_and_prints_usage() {
    let assert = specify().arg("--help").assert().success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        output.contains("specify") && output.contains("Usage"),
        "expected usage in stdout, got:\n{output}"
    );
}

#[test]
fn init_text_format_succeeds() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["init", "omnia", "--schema-dir"])
        .arg(repo_root())
        .args(["--name", "demo"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("Initialized"));
    assert!(stdout.contains("omnia"));
    assert!(stdout.contains(".specify/project.yaml"));

    let config_path = tmp.path().join(".specify/project.yaml");
    assert!(config_path.is_file(), "project.yaml must exist");
}

#[test]
fn init_json_format_has_stable_shape() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "init", "omnia", "--schema-dir"])
        .arg(repo_root())
        .args(["--name", "demo"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["schema_name"], "omnia");
    assert!(value["config_path"].is_string());
    let config_path = value["config_path"].as_str().unwrap();
    // Canonicalized tmp path so substring match handles macOS
    // /private/var symlinks gracefully.
    let canonical_tmp = fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    assert!(
        config_path.starts_with(canonical_tmp.to_string_lossy().as_ref()),
        "config_path {config_path} should start with {}",
        canonical_tmp.display()
    );
    assert!(value["specify_version"].is_string());
    assert!(value["scaffolded_rule_keys"].is_array());
}

#[test]
fn version_too_old_exits_three_with_json_envelope() {
    let tmp = tempdir().unwrap();
    // Fresh init to produce a real project.
    specify()
        .current_dir(tmp.path())
        .args(["init", "omnia", "--schema-dir"])
        .arg(repo_root())
        .args(["--name", "demo"])
        .assert()
        .success();

    // Pin a version far in the future.
    let config_path = tmp.path().join(".specify/project.yaml");
    let original = fs::read_to_string(&config_path).unwrap();
    let edited = original.replace(
        &format!("specify_version: {}", env!("CARGO_PKG_VERSION")),
        "specify_version: 99.0.0",
    );
    fs::write(&config_path, edited).unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "validate", "."])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("process exited with a code");
    assert_eq!(code, 3, "expected exit code 3 (version too old)");

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"], "specify_version_too_old");
    assert_eq!(value["exit_code"], 3);
}

// Change I's stub-subcommand assertion was retired in Change J; every
// subcommand now dispatches to real logic. End-to-end coverage of the
// wired subcommands lives in `tests/e2e.rs`.
