//! Cross-cutting binary-contract tests for the `specify` CLI.
//!
//! These are the subcommand-agnostic invariants every release must
//! preserve: the top-level `--help` shape, exit-code contracts that
//! aren't tied to a single subcommand, and JSON-envelope skeletons
//! that any verb may surface. Per-subcommand integration coverage
//! lives in dedicated `tests/<subcommand>.rs` files.

use std::fs;

use tempfile::tempdir;

use crate::common::{omnia_schema_dir, specify_cmd};

#[test]
fn help_exits_zero_and_prints_usage() {
    let assert = specify_cmd().arg("--help").assert().success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        output.contains("specify") && output.contains("Usage"),
        "expected usage in stdout, got:\n{output}"
    );
}

#[test]
fn version_too_old_exits_three_json() {
    // Generic exit-code 3 + JSON error-envelope contract: pin the
    // `specify-version-too-old` shape via a real project. Routed
    // through `slice validate` because that path runs the version
    // gate after a successful init; the gate itself is subcommand-
    // agnostic and the assertions below only touch the envelope.
    let tmp = tempdir().unwrap();
    // Fresh init to produce a real project.
    specify_cmd()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
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

    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "slice", "validate", "."])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("process exited with a code");
    assert_eq!(code, 3, "expected exit code 3 (version too old)");

    // Failure envelopes are written to stderr.
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stderr).expect("stderr is JSON");
    assert_eq!(value["error"], "specify-version-too-old");
    assert_eq!(value["exit-code"], 3);
}

#[test]
fn migration_required_exit_four_json() {
    // Exit-code 4 mirror of the exit-3 test above: pin the project a
    // full major *below* the binary. The migration gate compares majors,
    // so while the binary is 0.x the gate is dormant and the command
    // succeeds; from the 1.0 cut onward the same fixture must exit 4
    // with the `project-needs-migration` envelope — no test edit needed.
    // The wiring itself is unit-covered today via the injected-version
    // test `config::tests::load_refuses_migration_owed_pin`.
    let tmp = tempdir().unwrap();
    specify_cmd()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let config_path = tmp.path().join(".specify/project.yaml");
    let original = fs::read_to_string(&config_path).unwrap();
    let edited = original.replace(
        &format!("specify_version: {}", env!("CARGO_PKG_VERSION")),
        "specify_version: 0.0.1",
    );
    assert_ne!(original, edited, "fixture must repin specify_version");
    fs::write(&config_path, edited).unwrap();

    let binary_major: u64 = env!("CARGO_PKG_VERSION")
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .expect("binary version has a numeric major");

    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "slice", "validate", "."])
        .assert();
    if binary_major == 0 {
        // Same-major pin: the gate must NOT fire pre-1.0. The command
        // gets past config load and fails ordinary slice validation
        // (exit 2) — proof the refusal stayed dormant.
        let assert = assert.failure();
        let code = assert.get_output().status.code().expect("process exited with a code");
        assert_eq!(code, 2, "pre-1.0 the migration gate must not fire");
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
        let value: serde_json::Value = serde_json::from_str(&stderr).expect("stderr is JSON");
        assert_eq!(value["error"], "slice-validation-failed");
    } else {
        let assert = assert.failure();
        let code = assert.get_output().status.code().expect("process exited with a code");
        assert_eq!(code, 4, "expected exit code 4 (migration required)");
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
        let value: serde_json::Value = serde_json::from_str(&stderr).expect("stderr is JSON");
        assert_eq!(value["error"], "project-needs-migration");
        assert_eq!(value["exit-code"], 4);
    }
}
