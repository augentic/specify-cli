//! Integration tests for `specrun tool schema`.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

mod common;
use common::{parse_json, repo_root, sha256_hex, specify};

fn contract_wasm() -> PathBuf {
    repo_root().join("wasi-tools/contract/dist/contract-0.2.0.wasm")
}

fn vectis_wasm() -> PathBuf {
    repo_root().join("wasi-tools/target/wasm32-wasip2/release/vectis.wasm")
}

use std::sync::atomic::{AtomicU64, Ordering};

struct SchemaFixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    cache: PathBuf,
}

fn scaffold_project_with_tool(tool_name: &str, wasm_path: &Path) -> SchemaFixture {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);

    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().to_path_buf();
    let adapter = project.join("adapters/targets/test-adp");
    let briefs = adapter.join("briefs");
    fs::create_dir_all(project.join(".specify")).expect("create .specify");
    fs::create_dir_all(&briefs).expect("create adapter briefs");

    let cache = std::env::temp_dir()
        .join(format!("specify-tool-schema-{tool_name}-{}-{n}", std::process::id()));
    fs::create_dir_all(&cache).expect("create cache");

    fs::write(
        project.join(".specify/project.yaml"),
        "name: schema-test\nadapter: test-adp\nrules: {}\n",
    )
    .expect("write project.yaml");
    fs::write(
        adapter.join("adapter.yaml"),
        "name: test-adp\nversion: 1\naxis: target\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\ndescription: Test adapter\n",
    )
    .expect("write adapter.yaml");
    for op in ["shape", "build", "merge"] {
        fs::write(
            briefs.join(format!("{op}.md")),
            format!("---\nid: {op}\ndescription: {op} brief\n---\n"),
        )
        .expect("write brief");
    }

    let source = format!("file://{}", wasm_path.display());
    let sha256 = sha256_hex(wasm_path);
    fs::write(
        adapter.join("tools.yaml"),
        format!(
            "tools:\n  - name: {tool_name}\n    version: 0.1.0\n    source: \"{source}\"\n    sha256: \"{sha256}\"\n    permissions:\n      read: []\n      write: []\n"
        ),
    )
    .expect("write tools.yaml");

    SchemaFixture {
        _tmp: tmp,
        project,
        cache,
    }
}

#[test]
fn schema_contract_no_schemas_exits_nonzero() {
    let wasm = contract_wasm();
    assert!(
        wasm.is_file(),
        "contract WASM not found at {}; run `cargo make contract-wasm`",
        wasm.display()
    );

    let fixture = scaffold_project_with_tool("contract", &wasm);

    let assert = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
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

    let fixture = scaffold_project_with_tool("vectis", &wasm);

    let assert = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
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

    let fixture = scaffold_project_with_tool("vectis", &wasm);

    let assert = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
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
    fs::write(project.join(".specify/project.yaml"), "name: schema-test\nhub: true\ntools: []\n")
        .expect("write project.yaml");

    let cache =
        std::env::temp_dir().join(format!("specify-tool-schema-undeclared-{}", std::process::id()));
    fs::create_dir_all(&cache).expect("create cache");

    let assert = specify()
        .current_dir(project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["--format", "json", "tool", "schema", "nosuch", "tokens"])
        .assert()
        .failure();

    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "validation");
}

#[test]
fn help_lists_schema_verb() {
    let assert = specify().args(["tool", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("schema"), "tool --help must list `schema`, got:\n{stdout}");
}
