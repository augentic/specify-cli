//! Acceptance coverage for the first-party `contract` WASI tool.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};

mod common;
use common::{parse_json, repo_root, specify};

fn contract_wasm() -> PathBuf {
    repo_root().join("wasi-tools/contract/dist/contract-0.2.0.wasm")
}

fn sha256_hex(path: &Path) -> String {
    let bytes = fs::read(path).expect("read contract wasm");
    format!("{:x}", Sha256::digest(bytes))
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
        let capability = project.join("schemas/contracts");
        fs::create_dir_all(project.join(".specify")).expect("create .specify");
        fs::create_dir_all(project.join("contracts/http")).expect("create contracts");
        fs::create_dir_all(&capability).expect("create capability");

        fs::write(
            project.join(".specify/project.yaml"),
            "name: contract-tool-test\ncapability: contracts\nrules: {}\n",
        )
        .expect("write project.yaml");
        fs::write(
            capability.join("capability.yaml"),
            "name: contracts\nversion: 1\ndescription: Test contracts capability\npipeline:\n  define: []\n  build: []\n  merge: []\n",
        )
        .expect("write capability.yaml");

        let wasm = contract_wasm();
        let source = format!("file://{}", wasm.display());
        let sha256 = sha256_hex(&wasm);
        fs::write(
            capability.join("tools.yaml"),
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
fn lists_from_capability_sidecar() {
    let fixture = ContractToolFixture::new();

    let assert = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .args(["--format", "json", "tool", "list"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let tools = value["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "{value}");
    assert_eq!(tools[0]["name"], "contract");
    assert_eq!(tools[0]["version"], "0.2.0");
    assert_eq!(tools[0]["scope"], "capability");
    assert_eq!(tools[0]["scope-detail"], "contracts");
}

#[test]
fn preserves_validator_json_for_clean() {
    let fixture = ContractToolFixture::new();
    fixture.write_contract(
        "http/user-api.yaml",
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\n",
    );

    let assert = specify()
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
    assert_eq!(value["envelope-version"], 2);
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

    let assert = specify()
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
