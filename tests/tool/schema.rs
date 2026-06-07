//! Integration tests for `specify tool schema`.

use std::fs;
use std::path::PathBuf;

use tempfile::tempdir;

use crate::common::{parse_json, repo_root, scaffold_tool_project, specify_cmd};

fn contract_wasm() -> PathBuf {
    repo_root().join("wasi-tools/contract/dist/contract-0.2.0.wasm")
}

fn vectis_wasm() -> PathBuf {
    repo_root().join("target/vectis-wasi-tools/release/vectis.wasm")
}

#[test]
fn schema_contract_no_schemas_exits_nonzero() {
    let wasm = contract_wasm();
    assert!(
        wasm.is_file(),
        "contract WASM not found at {}; run `cargo make contract-wasm`",
        wasm.display()
    );

    let tmp = tempdir().expect("tempdir");
    let (project, cache) = scaffold_tool_project(&tmp, "contract", &wasm);

    let assert = specify_cmd()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "schema", "contract", "tokens"])
        .assert()
        .failure();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "no-schemas-declared");
    assert_eq!(value["exit-code"], 2);
}

#[test]
fn schema_vectis_tokens_returns_valid_json() {
    let wasm = vectis_wasm();
    if !wasm.is_file() {
        eprintln!(
            "skipping: vectis WASM not found at {}; run `cargo make vectis-wasm`",
            wasm.display()
        );
        return;
    }

    let tmp = tempdir().expect("tempdir");
    let (project, cache) = scaffold_tool_project(&tmp, "vectis", &wasm);

    let assert = specify_cmd()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "schema", "vectis", "tokens"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert!(value.is_object(), "output must be a JSON object");
    let id = value["$id"].as_str().expect("$id field must be present");
    assert!(id.contains("tokens"), "$id should reference 'tokens', got: {id}");
}

#[test]
fn schema_vectis_unknown_name_exits_nonzero() {
    let wasm = vectis_wasm();
    if !wasm.is_file() {
        eprintln!(
            "skipping: vectis WASM not found at {}; run `cargo make vectis-wasm`",
            wasm.display()
        );
        return;
    }

    let tmp = tempdir().expect("tempdir");
    let (project, cache) = scaffold_tool_project(&tmp, "vectis", &wasm);

    let assert = specify_cmd()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "schema", "vectis", "nonexistent"])
        .assert()
        .failure();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["error"], "unknown-schema");
    assert_eq!(value["exit-code"], 2);
}

#[test]
fn schema_undeclared_tool_exits_two() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path();
    fs::create_dir_all(project.join(".specify")).expect("create .specify");
    fs::write(
        project.join(".specify/project.yaml"),
        "name: schema-test\nworkspace: true\ntools: []\n",
    )
    .expect("write project.yaml");

    let cache =
        std::env::temp_dir().join(format!("specify-tool-schema-undeclared-{}", std::process::id()));
    fs::create_dir_all(&cache).expect("create cache");

    let assert = specify_cmd()
        .current_dir(project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["--format", "json", "tool", "schema", "nosuch", "tokens"])
        .assert()
        .failure();

    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "tool-not-declared");
}

#[test]
fn help_lists_schema_verb() {
    let assert = specify_cmd().args(["tool", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("schema"), "tool --help must list `schema`, got:\n{stdout}");
}
