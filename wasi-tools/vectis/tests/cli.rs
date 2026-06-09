//! CLI coverage for the `vectis` subcommand surface.

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn vectis() -> Command {
    Command::cargo_bin("vectis").expect("binary `vectis` is present")
}

fn vectis_validate() -> Command {
    let mut cmd = vectis();
    cmd.arg("validate");
    cmd
}

fn parse_json(stdout: &[u8]) -> Value {
    let s = String::from_utf8(stdout.to_vec()).expect("utf8 stdout");
    serde_json::from_str(&s).unwrap_or_else(|err| panic!("stdout is not JSON ({err}): {s}"))
}

#[test]
fn assets_clean_run_exits_zero() {
    let tmp = tempdir().unwrap();
    let assets_path = tmp.path().join("assets.yaml");
    std::fs::write(&assets_path, "version: 1\nassets: {}\n").expect("write assets.yaml");

    let assert = vectis_validate().args(["assets"]).arg(&assets_path).assert().success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["mode"], "assets");
    assert_eq!(value["path"], assets_path.display().to_string());
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0));
    assert_eq!(value["warnings"].as_array().map(Vec::len), Some(0));
}

#[test]
fn findings_exit_one_with_success_envelope() {
    let tmp = tempdir().unwrap();
    let tokens_path = tmp.path().join("tokens.yaml");
    std::fs::write(&tokens_path, ": : not valid yaml :::\n").expect("write tokens.yaml");

    let assert = vectis_validate().args(["tokens"]).arg(&tokens_path).assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["mode"], "tokens");
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(1));
    assert!(
        value["errors"][0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
        "unexpected error payload: {value}"
    );
}

#[test]
fn missing_input_exits_two() {
    let tmp = tempdir().unwrap();
    let missing = tmp.path().join("missing-tokens.yaml");

    let assert = vectis_validate().args(["tokens"]).arg(&missing).assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 2);
}

#[test]
fn invalid_invocation_exits_two() {
    vectis_validate().arg("nope").assert().failure().code(2);
}

#[test]
fn omitted_path_uses_default_root() {
    let tmp = tempdir().unwrap();
    let slice_dir = tmp.path().join(".specify/slices/active");
    std::fs::create_dir_all(&slice_dir).expect("mkdir slice");
    std::fs::write(slice_dir.join("layout.yaml"), "version: 1\nscreens: {}\n")
        .expect("write layout.yaml");

    let assert = vectis_validate().env("PROJECT_DIR", tmp.path()).arg("layout").assert().success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["mode"], "layout");
    let resolved = value["path"].as_str().expect("path is a string");
    assert!(
        resolved.ends_with(".specify/slices/active/layout.yaml"),
        "expected PROJECT_DIR default resolution, got: {resolved}"
    );
}

#[test]
fn all_mode_recurses_findings_exit_code() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    std::fs::write(design.join("tokens.yaml"), ": : not valid yaml :::\n")
        .expect("write tokens.yaml");

    let assert = vectis_validate().env("PROJECT_DIR", tmp.path()).arg("all").assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["mode"], "all");
    assert!(
        value["results"].as_array().expect("results array").iter().any(|entry| {
            entry["report"]["errors"].as_array().is_some_and(|errors| !errors.is_empty())
        }),
        "expected nested findings in all-mode payload: {value}"
    );
}

// ── verify subcommand ──────────────────────────────────────────────

fn vectis_verify() -> Command {
    let mut cmd = vectis();
    cmd.arg("verify");
    cmd
}

fn write_project_yaml(root: &std::path::Path, platforms: &[&str]) {
    let yaml_platforms: Vec<String> = platforms.iter().map(|p| format!("  - {p}")).collect();
    let content = format!(
        "name: test-app\nadapter: vectis\nspecify_version: '2.0'\nplatforms:\n{}",
        yaml_platforms.join("\n"),
    );
    std::fs::write(root.join("project.yaml"), content).expect("write project.yaml");
}

fn scaffold_core(root: &std::path::Path) {
    let dir = root.join("shared/src");
    std::fs::create_dir_all(&dir).expect("mkdir shared/src");
    std::fs::write(dir.join("app.rs"), "pub struct App;").expect("write app.rs");
}

fn scaffold_ios(root: &std::path::Path) {
    let dir = root.join("iOS/TestApp");
    std::fs::create_dir_all(&dir).expect("mkdir iOS/TestApp");
    std::fs::write(dir.join("ContentView.swift"), "struct ContentView {}").expect("write swift");
}

fn scaffold_android(root: &std::path::Path) {
    let dir = root.join("Android/app/src/main/kotlin/com/test");
    std::fs::create_dir_all(&dir).expect("mkdir Android");
    std::fs::write(dir.join("MainActivity.kt"), "class MainActivity").expect("write kt");
}

#[test]
fn verify_detect_all_present_exits_zero() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
    scaffold_core(tmp.path());
    scaffold_ios(tmp.path());
    scaffold_android(tmp.path());

    let assert = vectis_verify().args(["--mode", "detect"]).arg(tmp.path()).assert().success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["mode"], "detect");
    let missing = value["missing"].as_array().expect("missing array");
    assert!(missing.is_empty(), "expected empty missing: {value}");
}

#[test]
fn verify_detect_missing_shell_exits_zero_with_missing() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios"]);
    scaffold_core(tmp.path());

    let assert = vectis_verify().args(["--mode", "detect"]).arg(tmp.path()).assert().success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["mode"], "detect");
    let missing = value["missing"].as_array().expect("missing array");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], "ios");
}

#[test]
fn verify_verify_missing_shell_exits_one() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "android"]);
    scaffold_core(tmp.path());

    let assert = vectis_verify().args(["--mode", "verify"]).arg(tmp.path()).assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["mode"], "verify");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(findings.iter().any(|f| f["id"] == "platform-shell-missing"));
}

#[test]
fn verify_missing_project_yaml_exits_two() {
    let tmp = tempdir().unwrap();

    let assert = vectis_verify().args(["--mode", "detect"]).arg(tmp.path()).assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 2);
}

#[test]
fn verify_uses_project_dir_env() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core"]);
    scaffold_core(tmp.path());

    let assert = vectis_verify()
        .env("PROJECT_DIR", tmp.path())
        .args(["--mode", "detect"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["mode"], "detect");
    let missing = value["missing"].as_array().expect("missing array");
    assert!(missing.is_empty());
}

// ── schema subcommand ──────────────────────────────────────────────

fn vectis_schema() -> Command {
    let mut cmd = vectis();
    cmd.arg("schema");
    cmd
}

#[test]
fn schema_tokens_exits_zero_with_valid_json() {
    let assert = vectis_schema().arg("tokens").assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["$id"].as_str().is_some(), "$id field must be present");
    assert_eq!(value["title"], "Specify Tokens Artifact");
}

#[test]
fn schema_assets_exits_zero_with_valid_json() {
    let assert = vectis_schema().arg("assets").assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["$id"].as_str().is_some_and(|id| id.contains("assets")));
}

#[test]
fn schema_composition_exits_zero_with_valid_json() {
    let assert = vectis_schema().arg("composition").assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["$id"].as_str().is_some_and(|id| id.contains("composition")));
}

#[test]
fn schema_unknown_exits_two_with_error_envelope() {
    let assert = vectis_schema().arg("nonexistent").assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["error"], "unknown-schema");
    assert_eq!(value["exit-code"], 2);
    assert!(
        value["message"].as_str().unwrap_or("").contains("nonexistent"),
        "error message should mention the requested name"
    );
}

// ── infer subcommand ───────────────────────────────────────────────

#[test]
fn infer_emits_name_free_cluster_report() {
    let tmp = tempdir().unwrap();
    let comp = tmp.path().join("composition.yaml");
    std::fs::write(
        &comp,
        "version: 1\nscreens:\n  home:\n    name: Home\n    footer:\n      - group:\n          items:\n            - icon-button: {}\n            - icon-button: {}\n  search:\n    name: Search\n    footer:\n      - group:\n          items:\n            - icon-button: {}\n            - icon-button: {}\n",
    )
    .expect("write composition.yaml");

    let assert = vectis().args(["infer", "--composition"]).arg(&comp).assert().success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["version"], 1);
    let clusters = value["clusters"].as_array().expect("clusters array");
    assert_eq!(clusters.len(), 1, "expected one cluster: {value}");
    assert_eq!(clusters[0]["occurrences"], 2);
    assert_eq!(clusters[0]["bound-slug"], Value::Null);
    assert!(clusters[0]["fingerprint"].as_str().is_some(), "cluster carries a fingerprint");
}

#[test]
fn infer_missing_composition_exits_two() {
    let tmp = tempdir().unwrap();
    let missing = tmp.path().join("composition.yaml");

    let assert = vectis().args(["infer", "--composition"]).arg(&missing).assert().failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 2);
}
