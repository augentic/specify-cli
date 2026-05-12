//! CLI coverage for the `vectis validate` subcommand surface.

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn vectis_validate() -> Command {
    let mut cmd = Command::cargo_bin("vectis").expect("binary `vectis` is present");
    cmd.arg("validate");
    cmd
}

fn parse_json(stdout: &[u8]) -> Value {
    let s = String::from_utf8(stdout.to_vec()).expect("utf8 stdout");
    serde_json::from_str(&s).unwrap_or_else(|err| panic!("stdout is not JSON ({err}): {s}"))
}

#[test]
fn assets_clean_run_exits_zero_with_v2_envelope() {
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
fn missing_input_exits_two_with_error_envelope() {
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
fn omitted_path_uses_project_dir_default_root() {
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
