//! Integration tests for the `specify vectis` subcommand tree.
//!
//! These lock in the v2 JSON contract for the four `vectis` verbs that
//! ship under the `specify` binary (chunk 5 of
//! `docs/plans/fold-vectis-into-specify.md`). The tests deliberately
//! exercise the whole `specify` binary end-to-end via `assert_cmd`
//! rather than calling into the `specify-vectis` library directly so
//! the global `--format` flag, `emit_json` envelope, and
//! `emit_vectis_error` mapping are all in scope.
//!
//! Where a test depends on workstation toolchain (`vectis init` needs
//! `rustup`/`cargo-deny`/`cargo-vet` to be on PATH), we soft-skip when
//! the binary reports `missing-prerequisites` so the suite stays green
//! on a stripped-down CI host. The dedicated
//! `init_missing_prereqs_json_shape` test goes the other way: it
//! *forces* the missing-prereqs path by clearing PATH so the JSON shape
//! is asserted unconditionally.

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn parse_json(stdout: &[u8]) -> Value {
    let s = String::from_utf8(stdout.to_vec()).expect("utf8 stdout");
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {s}"))
}

#[test]
fn vectis_help_lists_four_subcommands() {
    let assert = specify().args(["vectis", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["init", "verify", "add-shell", "update-versions"] {
        assert!(
            stdout.contains(verb),
            "expected `vectis --help` to mention {verb}, got:\n{stdout}"
        );
    }
}

/// `init` happy path: assert the exact top-level kebab-case key set
/// plus the auto-injected `schema-version`. Soft-skips when the host
/// is missing the core toolchain (CI hosts without `rustup` etc.) so
/// the assertion below fires only when we actually got the success
/// payload.
#[test]
fn init_success_json_has_kebab_keys_and_schema_version() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .args(["--format", "json", "vectis", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .assert();
    let output = assert.get_output();
    let stdout = output.stdout.clone();
    let value = parse_json(&stdout);

    if value.get("error").and_then(Value::as_str) == Some("missing-prerequisites") {
        eprintln!(
            "skipping init success test: workstation lacks core prereqs ({})",
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("(no message)")
        );
        return;
    }

    assert!(
        output.status.success(),
        "expected success, got status {:?} and stdout:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&stdout)
    );

    assert_eq!(
        value.get("schema-version"),
        Some(&Value::from(2)),
        "missing schema-version: {value}"
    );

    let map = value.as_object().expect("top-level JSON is an object");
    let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "app-name",
            "app-struct",
            "assemblies",
            "capabilities",
            "project-dir",
            "schema-version",
            "shells",
        ],
        "init success payload key set drifted (chunk 4 invariant): {value}"
    );

    assert_eq!(value["app-name"], "Foo");
    assert_eq!(value["app-struct"], "Foo");
    let project_dir = value["project-dir"]
        .as_str()
        .expect("project-dir is a string");
    let canonical_tmp = std::fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    let canonical_project = std::fs::canonicalize(PathBuf::from(project_dir))
        .expect("canonicalize project-dir");
    assert_eq!(canonical_project, canonical_tmp);

    let core = value["assemblies"]
        .get("core")
        .expect("`core` assembly present");
    assert_eq!(core["status"], "created");
    assert!(core["files"].is_array(), "core.files is an array");
}

/// `init` with a `--version-file` pointing at a missing path: the v2
/// error envelope must report `invalid-project` with `exit-code: 1`.
/// This path is independent of workstation toolchain, so it runs
/// unconditionally.
#[test]
fn init_invalid_project_json_shape() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .args(["--format", "json", "vectis", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .args(["--version-file", "/tmp/specify-vectis-does-not-exist.toml"])
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["schema-version"], 2);
    assert!(
        value["message"]
            .as_str()
            .unwrap_or("")
            .contains("version file not found"),
        "unexpected message: {value}"
    );
    assert_eq!(output.status.code(), Some(1));
}

/// Force the `missing-prerequisites` path by clearing PATH so every
/// `Command::new("rustup")` etc. lookup fails with ENOENT. The binary
/// itself is launched via an absolute path by `assert_cmd` so the
/// process still starts.
#[test]
fn init_missing_prereqs_json_shape() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .env("PATH", "")
        .env_remove("CARGO_HOME")
        .env_remove("RUSTUP_HOME")
        .args(["--format", "json", "vectis", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "missing-prerequisites");
    assert_eq!(value["exit-code"], 2);
    assert_eq!(value["schema-version"], 2);
    let missing = value["missing"]
        .as_array()
        .expect("missing is an array");
    assert!(
        !missing.is_empty(),
        "expected at least one missing tool with PATH cleared: {value}"
    );
    let first = &missing[0];
    for field in ["tool", "assembly", "check", "install"] {
        assert!(
            first.get(field).and_then(Value::as_str).is_some(),
            "missing[0].{field} should be a string: {value}"
        );
    }
    assert_eq!(output.status.code(), Some(2));
}
