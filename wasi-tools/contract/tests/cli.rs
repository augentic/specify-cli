//! Integration tests for the standalone `specify-contract` binary.
//!
//! Each test materialises a synthetic baseline under a `TempDir`, runs
//! the binary with `assert_cmd`, and asserts on exit code + stdout.
//! The JSON output is parsed back into `serde_json::Value` for
//! structural assertions, while the byte sequence is also pinned by a
//! field-order check so any future drift in the envelope shape fails
//! loud.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

/// Materialise `<tmp>/contracts/<rel>` with the supplied YAML body.
fn write_contract(tmp: &TempDir, rel: &str, body: &str) -> PathBuf {
    let path = tmp.path().join("contracts").join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    path
}

fn contracts_dir(tmp: &TempDir) -> PathBuf {
    tmp.path().join("contracts")
}

fn cmd() -> Command {
    Command::cargo_bin("specify-contract").expect("binary `specify-contract` is present")
}

#[test]
fn clean_baseline_exits_zero_with_empty_findings() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\n",
    );

    let assert = cmd().arg(contracts_dir(&tmp)).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["envelope-version"], 2);
    assert_eq!(value["ok"], true);
    assert_eq!(value["findings"], serde_json::json!([]));
    assert_eq!(value["exit-code"], 0);
    assert_eq!(value["contracts-dir"], contracts_dir(&tmp).display().to_string());
}

#[test]
fn semver_violation_exits_one_with_finding_in_json() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
    );

    let assert = cmd().arg(contracts_dir(&tmp)).assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(value["ok"], false);
    assert_eq!(value["exit-code"], 1);
    let findings = value["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "single semver violation");
    assert_eq!(findings[0]["rule-id"], "contract.version-is-semver");
    assert_eq!(findings[0]["path"], "contracts/http/user-api.yaml");
    let detail = findings[0]["detail"].as_str().expect("detail string");
    assert!(detail.contains("2024-01-15"), "detail mentions offending version");
}

#[test]
fn duplicate_x_specify_id_flags_both_files() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: shared\n",
    );
    write_contract(
        &tmp,
        "http/billing-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: Billing API\n  version: 1.0.0\n  x-specify-id: shared\n",
    );

    let assert = cmd().arg(contracts_dir(&tmp)).assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");

    let findings = value["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 2);
    assert!(findings.iter().all(|f| f["rule-id"] == "contract.id-unique"));
}

#[test]
fn missing_baseline_dir_exits_two() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("does-not-exist");

    let assert = cmd().arg(&missing).assert().code(2);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("baseline directory does not exist"),
        "diagnostic mentions missing baseline: {stderr}"
    );
}

#[test]
fn baseline_is_a_file_exits_two() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("not-a-dir.txt");
    fs::write(&file, "hello").unwrap();

    let assert = cmd().arg(&file).assert().code(2);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("not a directory"), "diagnostic mentions non-directory: {stderr}");
}

#[test]
fn text_format_pass_summary() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n",
    );

    let assert = cmd().arg(contracts_dir(&tmp)).arg("--format").arg("text").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.starts_with("PASS"), "text PASS preamble: {stdout}");
}

#[test]
fn text_format_fail_lists_findings_on_stderr() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
    );

    let assert = cmd().arg(contracts_dir(&tmp)).arg("--format").arg("text").assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stdout.starts_with("FAIL"), "text FAIL preamble: {stdout}");
    assert!(stderr.contains("contract.version-is-semver"), "rule id surfaces on stderr: {stderr}");
}

/// Pin the envelope key order on the JSON output. Operator scripts
/// parsing line-by-line (e.g. `head` + `grep`) rely on
/// `envelope-version` being first and `findings`/`exit-code` being
/// last; the typed `Serialize` structs in
/// `specify_validate::serialize_contract_findings` lock that order in.
#[test]
fn json_envelope_preserves_field_order() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
    );

    let assert = cmd().arg(contracts_dir(&tmp)).assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    let p_schema = stdout.find("\"envelope-version\"").expect("envelope-version present");
    let p_contracts = stdout.find("\"contracts-dir\"").expect("contracts-dir present");
    let p_ok = stdout.find("\"ok\"").expect("ok present");
    let p_findings = stdout.find("\"findings\"").expect("findings present");
    let p_exit = stdout.find("\"exit-code\"").expect("exit-code present");
    assert!(p_schema < p_contracts);
    assert!(p_contracts < p_ok);
    assert!(p_ok < p_findings);
    assert!(p_findings < p_exit);

    let p_path = stdout.find("\"path\"").expect("path present");
    let p_rule = stdout.find("\"rule-id\"").expect("rule-id present");
    let p_detail = stdout.find("\"detail\"").expect("detail present");
    assert!(p_path < p_rule);
    assert!(p_rule < p_detail);
}

/// Pin the **byte sequence** of the JSON envelope on a known
/// fixture. Any future drift from the wire shape fails this golden
/// snapshot.
///
/// Volatile parts of the path (the tempdir prefix) are masked out
/// with `<TMP>` before comparison so the snapshot is portable.
#[test]
fn json_envelope_matches_byte_sequence() {
    let tmp = TempDir::new().unwrap();
    write_contract(
        &tmp,
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
    );
    let baseline = contracts_dir(&tmp);

    let assert = cmd().arg(&baseline).assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    let tmp_str = tmp.path().display().to_string();
    let masked = stdout.replace(&tmp_str, "<TMP>");

    let expected = "{\n  \
        \"envelope-version\": 2,\n  \
        \"contracts-dir\": \"<TMP>/contracts\",\n  \
        \"ok\": false,\n  \
        \"findings\": [\n    \
            {\n      \
                \"path\": \"contracts/http/user-api.yaml\",\n      \
                \"rule-id\": \"contract.version-is-semver\",\n      \
                \"detail\": \"info.version `2024-01-15` is not valid SemVer (must parse per semver.org, including optional prerelease labels)\"\n    \
            }\n  \
        ],\n  \
        \"exit-code\": 1\n\
        }\n";
    assert_eq!(masked, expected);
}
