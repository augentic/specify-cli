//! Integration tests for the standalone `specify-vectis` binary
//! (RFC-13 §4.3a).
//!
//! These pin the v2 JSON envelope shape that the pre-2.6
//! `specify vectis * --format json` dispatcher produced — the parity
//! contract the chunk-4.3a acceptance gate calls out byte-for-byte. The
//! historical end-to-end coverage of the same shape lived in
//! `tests/vectis.rs` against the `specify` binary; chunk 2.6 retired
//! `Commands::Vectis` and this file rehomes the assertions against the
//! standalone binary.
//!
//! Where a test depends on workstation toolchain (`vectis init` /
//! `verify` / `add-shell` need `rustup`/`cargo-deny`/`cargo-vet` etc.
//! on PATH), we soft-skip when the binary reports
//! `missing-prerequisites` so the suite stays green on a stripped-down
//! CI host. The dedicated `*_missing_prereqs_json_shape` tests go the
//! other way: they *force* the missing-prereqs path by clearing PATH
//! and assert the JSON shape unconditionally.

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn vectis() -> Command {
    Command::cargo_bin("specify-vectis")
        .expect("binary `specify-vectis` is present in the workspace")
}

fn parse_json(stdout: &[u8]) -> Value {
    let s = String::from_utf8(stdout.to_vec()).expect("utf8 stdout");
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {s}"))
}

#[test]
fn help_lists_all_five_verbs() {
    let assert = vectis().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["init", "verify", "add-shell", "update-versions", "versions"] {
        assert!(
            stdout.contains(verb),
            "expected `specify-vectis --help` to mention {verb}, got:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("Usage: specify-vectis"),
        "expected canonical binary name in usage line: {stdout}"
    );
}

/// `versions` does not call `prerequisites::check`, so it runs cleanly
/// on every host. The output must carry the v2 envelope (`schema-version: 2`
/// first) and the four canonical sections.
#[test]
fn versions_emits_v2_envelope_with_pinned_sections() {
    let tmp = tempdir().unwrap();
    let assert = vectis().args(["versions", "--dir"]).arg(tmp.path()).assert().success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(
        value.get("schema-version"),
        Some(&Value::from(2)),
        "missing or wrong schema-version: {value}"
    );

    for section in ["crux", "android", "ios", "tooling"] {
        assert!(
            value.get(section).is_some(),
            "missing `{section}` section in versions payload: {value}"
        );
    }
    assert!(value["crux"]["crux_core"].is_string(), "crux.crux_core must be present");
}

/// `init` happy path: assert the exact top-level kebab-case key set
/// plus the auto-injected `schema-version`. Soft-skips when the host
/// is missing the core toolchain (CI hosts without `rustup` etc.) so
/// the assertion below fires only when we actually got the success
/// payload.
#[test]
fn init_success_json_has_kebab_keys_and_schema_version() {
    let tmp = tempdir().unwrap();
    let assert = vectis().args(["init", "Foo", "--dir"]).arg(tmp.path()).assert();
    let output = assert.get_output();
    let stdout = output.stdout.clone();
    let value = parse_json(&stdout);

    if value.get("error").and_then(Value::as_str) == Some("missing-prerequisites") {
        eprintln!(
            "skipping init success test: workstation lacks core prereqs ({})",
            value.get("message").and_then(Value::as_str).unwrap_or("(no message)")
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
        "init success payload key set drifted (chunk 4.3a parity invariant): {value}"
    );

    assert_eq!(value["app-name"], "Foo");
    assert_eq!(value["app-struct"], "Foo");
    let project_dir = value["project-dir"].as_str().expect("project-dir is a string");
    let canonical_tmp = std::fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    let canonical_project =
        std::fs::canonicalize(PathBuf::from(project_dir)).expect("canonicalize project-dir");
    assert_eq!(canonical_project, canonical_tmp);

    let core = value["assemblies"].get("core").expect("`core` assembly present");
    assert_eq!(core["status"], "created");
    assert!(core["files"].is_array(), "core.files is an array");
}

/// `init` with a `--version-file` pointing at a missing path: the v2
/// error envelope must report `invalid-project` with `exit-code: 1`.
/// This path is independent of workstation toolchain (it short-circuits
/// before `prerequisites::check`), so it runs unconditionally.
#[test]
fn init_invalid_project_json_shape() {
    let tmp = tempdir().unwrap();
    // Build the bogus version-file path *inside* tempdir so it's
    // guaranteed nonexistent on every platform without colliding with
    // anything else under /tmp.
    let missing = tmp.path().join("definitely-not-there.toml");
    let assert = vectis()
        .args(["init", "Foo", "--dir"])
        .arg(tmp.path())
        .arg("--version-file")
        .arg(&missing)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["schema-version"], 2);
    assert!(
        value["message"].as_str().unwrap_or("").contains("version file not found"),
        "unexpected message: {value}"
    );
    assert_eq!(output.status.code(), Some(1));
}

/// Force the `missing-prerequisites` path on `init` by clearing PATH
/// so every `Command::new("rustup")` etc. lookup fails with ENOENT.
/// `assert_cmd` launches the binary itself via an absolute path, so
/// the process still starts.
#[test]
fn init_missing_prereqs_json_shape() {
    let tmp = tempdir().unwrap();
    let assert = vectis()
        .env("PATH", "")
        .env_remove("CARGO_HOME")
        .env_remove("RUSTUP_HOME")
        .args(["init", "Foo", "--dir"])
        .arg(tmp.path())
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "missing-prerequisites");
    assert_eq!(value["exit-code"], 2);
    assert_eq!(value["schema-version"], 2);
    let missing = value["missing"].as_array().expect("missing is an array");
    assert!(!missing.is_empty(), "expected at least one missing tool with PATH cleared: {value}");
    let first = &missing[0];
    for field in ["tool", "assembly", "check", "install"] {
        assert!(
            first.get(field).and_then(Value::as_str).is_some(),
            "missing[0].{field} should be a string: {value}"
        );
    }
    assert_eq!(output.status.code(), Some(2));
}

/// Same forced missing-prereqs path against `verify` — confirms the
/// canonical "smoke" verb the chunk plan calls out goes through the
/// same envelope and exit-code mapping as `init` for the bad-fixture
/// case (CI hosts lacking the core toolchain).
#[test]
fn verify_missing_prereqs_json_shape() {
    let tmp = tempdir().unwrap();
    let assert = vectis()
        .env("PATH", "")
        .env_remove("CARGO_HOME")
        .env_remove("RUSTUP_HOME")
        .args(["verify", "--dir"])
        .arg(tmp.path())
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "missing-prerequisites");
    assert_eq!(value["exit-code"], 2);
    assert_eq!(value["schema-version"], 2);
    assert_eq!(output.status.code(), Some(2));
}

/// `verify` against a known-bad fixture (an empty project directory,
/// with PATH stripped of the core toolchain so the result is
/// deterministic on every host) emits the v2 error envelope with the
/// canonical key order — `schema-version` first, then the variant
/// payload, with `exit-code` appended at the end.
///
/// This is the chunk-4.3a parity check the prompt asks for: any future
/// drift from the legacy `specify vectis verify --format json` shape
/// fails this byte-sequence comparison. The dynamic `reason` strings
/// (which include OS-specific phrasing of "No such file or directory")
/// are masked out before comparison so the snapshot is portable across
/// linux / darwin / windows hosts.
#[test]
fn verify_bad_fixture_envelope_byte_sequence_parity() {
    let tmp = tempdir().unwrap();
    let assert = vectis()
        .env("PATH", "")
        .env_remove("CARGO_HOME")
        .env_remove("RUSTUP_HOME")
        .args(["--format", "json", "verify", "--dir"])
        .arg(tmp.path())
        .assert()
        .failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");

    // The envelope's structural skeleton must match the legacy shape
    // byte-for-byte: `schema-version` first, then `error`, then the
    // alphabetically-ordered remaining keys (`exit-code`, `message`,
    // `missing`). `serde_json::Map` defaults to a `BTreeMap` (no
    // `preserve_order` feature) on both the pre-2.6 dispatcher and
    // this binary, so this ordering is locked at the type level.
    let p_schema = stdout.find("\"schema-version\"").expect("schema-version present");
    let p_error = stdout.find("\"error\"").expect("error present");
    let p_exit = stdout.find("\"exit-code\"").expect("exit-code present");
    let p_message = stdout.find("\"message\"").expect("message present");
    let p_missing = stdout.find("\"missing\"").expect("missing present");
    assert!(p_schema < p_error, "schema-version must precede error: {stdout}");
    assert!(p_error < p_exit, "error must precede exit-code: {stdout}");
    assert!(p_exit < p_message, "exit-code must precede message: {stdout}");
    assert!(p_message < p_missing, "message must precede missing: {stdout}");

    // Assert the prefix and outer envelope shape verbatim. We pin the
    // pretty-printed indentation, key-quoting, and the `error`/
    // `exit-code`/`message` triple's textual form. The `missing` array
    // body and trailing `}` are matched separately because individual
    // tool entries carry OS-specific `reason` strings.
    assert!(
        stdout.starts_with(
            "{\n  \
             \"schema-version\": 2,\n  \
             \"error\": \"missing-prerequisites\",\n  \
             \"exit-code\": 2,\n  \
             \"message\": \"Install the missing tools above and re-run the command.\",\n  \
             \"missing\": [\n"
        ),
        "envelope prefix drifted from legacy shape:\n{stdout}"
    );
    assert!(stdout.trim_end().ends_with("]\n}"), "envelope tail drifted:\n{stdout}");

    // The first `missing` entry must carry the four kebab-case fields
    // the v2 contract pins (`tool`, `assembly`, `check`, `install`),
    // serialised in alphabetical order from `MissingTool`'s serde
    // default. The optional `reason` field (only when present) sorts
    // after `install`.
    let value = parse_json(assert.get_output().stdout.as_slice());
    let first = &value["missing"][0];
    for field in ["tool", "assembly", "check", "install"] {
        assert!(
            first.get(field).and_then(Value::as_str).is_some(),
            "missing[0].{field} drifted: {value}"
        );
    }
}

/// Text format default suppression check: when invoked with explicit
/// `--format text`, a `missing-prerequisites` failure renders the
/// human-readable summary on stderr (not the JSON envelope on stdout).
/// The chunk-4.3a parity contract is JSON-only, but we still cover
/// `--format text` so the binary's full UX surface stays exercised.
#[test]
fn text_format_renders_missing_prereqs_on_stderr() {
    let tmp = tempdir().unwrap();
    let assert = vectis()
        .env("PATH", "")
        .env_remove("CARGO_HOME")
        .env_remove("RUSTUP_HOME")
        .args(["--format", "text", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .assert()
        .failure();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8");
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8");

    assert!(
        stdout.is_empty() || !stdout.starts_with('{'),
        "stdout must not carry a JSON envelope under --format text: {stdout}"
    );
    assert!(
        stderr.contains("missing prerequisites"),
        "expected text-mode diagnostic on stderr: {stderr}"
    );
    assert_eq!(output.status.code(), Some(2));
}
