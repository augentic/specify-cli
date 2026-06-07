//! Acceptance coverage for the first-party `contract` WASI tool.

use std::fs;
use std::path::PathBuf;

use tempfile::{TempDir, tempdir};

use crate::common::{parse_json, repo_root, sha256_hex, specify_cmd};

fn contract_wasm() -> PathBuf {
    repo_root().join("wasi-tools/contract/dist/contract-0.2.0.wasm")
}

struct ContractToolFixture {
    _tmp: TempDir,
    project: PathBuf,
    cache: PathBuf,
}

impl ContractToolFixture {
    fn new() -> Self {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let adapter = project.join("adapters").join("targets/contracts");
        let briefs = adapter.join("briefs");
        fs::create_dir_all(project.join(".specify")).expect("create .specify");
        fs::create_dir_all(project.join("contracts/http")).expect("create contracts");
        fs::create_dir_all(&briefs).expect("create adapter briefs");

        fs::write(
            project.join(".specify/project.yaml"),
            "name: contract-tool-test\nadapter: contracts\nrules: {}\n",
        )
        .expect("write project.yaml");
        fs::write(
            adapter.join("adapter.yaml"),
            "name: contracts\nversion: 1\naxis: target\nexecution: agent\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\ndescription: Test contracts adapter\n",
        )
        .expect("write adapter.yaml");
        for op in ["shape", "build", "merge"] {
            fs::write(
                briefs.join(format!("{op}.md")),
                format!("---\nid: {op}\ndescription: {op} brief\n---\n"),
            )
            .expect("write brief");
        }

        let wasm = contract_wasm();
        let source = format!("file://{}", wasm.display());
        let sha256 = sha256_hex(&wasm);
        fs::write(
            adapter.join("tools.yaml"),
            format!(
                "tools:\n  - name: contract\n    version: 0.2.0\n    source: \"{source}\"\n    sha256: \"{sha256}\"\n    permissions:\n      read:\n        - \"$PROJECT_DIR/contracts\"\n      write: []\n"
            ),
        )
        .expect("write tools.yaml");

        let cache = tmp.path().join("tools-cache");
        fs::create_dir_all(&cache).expect("create cache");
        Self {
            _tmp: tmp,
            project,
            cache,
        }
    }

    fn contracts_dir(&self) -> PathBuf {
        self.project.join("contracts").canonicalize().expect("canonical contracts dir")
    }

    fn write_contract(&self, rel: &str, body: &str) {
        let path = self.contracts_dir().join(rel);
        fs::create_dir_all(path.parent().expect("contract parent")).expect("create contract dir");
        fs::write(path, body).expect("write contract");
    }
}

#[test]
fn preserves_validator_json_for_clean() {
    let fixture = ContractToolFixture::new();
    fixture.write_contract(
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\n",
    );

    let assert = specify_cmd()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .arg("tool")
        .arg("run")
        .arg("contract")
        .arg("--")
        .arg(fixture.contracts_dir())
        .args(["--format", "json"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["contracts-dir"], fixture.contracts_dir().display().to_string());
    assert_eq!(value["ok"], true);
    assert_eq!(value["findings"], serde_json::json!([]));
    assert_eq!(value["exit-code"], 0);
}

#[test]
fn preserves_validator_findings_exit_code() {
    let fixture = ContractToolFixture::new();
    fixture.write_contract(
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
    );

    let assert = specify_cmd()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .arg("tool")
        .arg("run")
        .arg("contract")
        .arg("--")
        .arg(fixture.contracts_dir())
        .args(["--format", "json"])
        .assert()
        .code(1);
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["ok"], false);
    assert_eq!(value["exit-code"], 1);
    let findings = value["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "{value}");
    assert_eq!(findings[0]["path"], "contracts/http/user-api.yaml");
    assert_eq!(findings[0]["rule-id"], "contract.version-is-semver");
}
