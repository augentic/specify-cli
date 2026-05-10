//! Integration coverage for Vectis tools declared through `specify tool`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};

mod common;
use common::{parse_json, repo_root, specify};

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn sha256_hex(path: &Path) -> String {
    let bytes = fs::read(path).expect("read wasm bytes for sha256");
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug)]
struct VectisWasiArtifacts {
    validate: PathBuf,
    scaffold: PathBuf,
}

fn vectis_wasi_artifacts() -> &'static VectisWasiArtifacts {
    static ARTIFACTS: OnceLock<VectisWasiArtifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let root = repo_root();
        let dist = root.join("target/vectis-wasi-tools/release");
        let validate = dist.join("vectis-validate.wasm");
        let scaffold = dist.join("vectis-scaffold.wasm");
        if !validate.is_file() || !scaffold.is_file() {
            build_vectis_wasi_artifacts(&root, &dist, &validate, &scaffold);
        }
        assert!(
            validate.is_file(),
            "missing Vectis validator WASI artifact at {}",
            validate.display()
        );
        assert!(
            scaffold.is_file(),
            "missing Vectis scaffold WASI artifact at {}",
            scaffold.display()
        );
        VectisWasiArtifacts { validate, scaffold }
    })
}

fn build_vectis_wasi_artifacts(root: &Path, dist: &Path, validate: &Path, scaffold: &Path) {
    let status = ProcessCommand::new("cargo")
        .current_dir(root)
        .args([
            "build",
            "-p",
            "vectis-validate",
            "-p",
            "vectis-scaffold",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--locked",
        ])
        .status()
        .unwrap_or_else(|err| panic!("failed to invoke cargo for Vectis WASI artifacts: {err}"));
    assert!(
        status.success(),
        "failed to build Vectis WASI artifacts with `cargo build -p vectis-validate -p vectis-scaffold --target wasm32-wasip2 --release --locked`"
    );

    fs::create_dir_all(dist).expect("create Vectis WASI dist dir");
    fs::copy(root.join("target/wasm32-wasip2/release/vectis-validate.wasm"), validate)
        .expect("copy vectis-validate.wasm to dist");
    fs::copy(root.join("target/wasm32-wasip2/release/vectis-scaffold.wasm"), scaffold)
        .expect("copy vectis-scaffold.wasm to dist");
}

struct VectisToolFixture {
    _tmp: TempDir,
    project: PathBuf,
    cache: PathBuf,
}

impl VectisToolFixture {
    fn from_tempdir(tmp: TempDir, scaffold_write_permission: &str) -> Self {
        let project = tmp.path().join("project");
        let capability = project.join("schemas/vectis");
        let design = project.join("design-system");
        let cache = tmp.path().join("tools-cache");
        let outside = tmp.path().join("outside");

        fs::create_dir_all(project.join(".specify")).expect("create .specify");
        fs::create_dir_all(&capability).expect("create capability");
        fs::create_dir_all(&design).expect("create design-system");
        fs::create_dir_all(&cache).expect("create cache");
        fs::create_dir_all(&outside).expect("create outside dir");

        fs::write(
            project.join(".specify/project.yaml"),
            "name: vectis-tool-test\ncapability: vectis\nrules: {}\n",
        )
        .expect("write project.yaml");
        fs::write(
            capability.join("capability.yaml"),
            "name: vectis\nversion: 2\ndescription: Test Vectis capability\npipeline:\n  define: []\n  build: []\n  merge: []\n",
        )
        .expect("write capability.yaml");

        let artifacts = vectis_wasi_artifacts();
        let validate_source = file_uri(&artifacts.validate);
        let scaffold_source = file_uri(&artifacts.scaffold);
        let validate_sha = sha256_hex(&artifacts.validate);
        let scaffold_sha = sha256_hex(&artifacts.scaffold);
        fs::write(
            capability.join("tools.yaml"),
            format!(
                "tools:\n  - name: vectis-validate\n    version: 0.2.0\n    source: \"{validate_source}\"\n    sha256: \"{validate_sha}\"\n    permissions:\n      read:\n        - \"$PROJECT_DIR/design-system\"\n      write: []\n  - name: vectis-scaffold\n    version: 0.2.0\n    source: \"{scaffold_source}\"\n    sha256: \"{scaffold_sha}\"\n    permissions:\n      read: []\n      write:\n        - \"{scaffold_write_permission}\"\n"
            ),
        )
        .expect("write tools.yaml");

        Self {
            _tmp: tmp,
            project,
            cache,
        }
    }

    fn with_project_write() -> Self {
        let tmp = tempdir().expect("tempdir");
        Self::from_tempdir(tmp, "$PROJECT_DIR")
    }

    fn with_scaffold_write_outside_project() -> Self {
        let tmp = tempdir().expect("tempdir");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).expect("create outside permission target");
        Self::from_tempdir(tmp, &outside.display().to_string())
    }

    fn write_tokens(&self, name: &str, body: &str) -> PathBuf {
        let path = self.project.join("design-system").join(name);
        fs::write(&path, body).expect("write tokens fixture");
        path.canonicalize().expect("canonical tokens fixture")
    }
}

fn run_json(project: &Path, cache: &Path, args: &[&str]) -> Value {
    let assert = specify()
        .current_dir(project)
        .env("SPECIFY_TOOLS_CACHE", cache)
        .args(["--format", "json"])
        .args(args)
        .assert()
        .success();
    parse_json(&assert.get_output().stdout)
}

fn assert_scaffold_run_and_permission_denial(fixture: &VectisToolFixture) {
    let scaffold = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .args(["tool", "run", "vectis-scaffold", "--", "core", "Counter"])
        .assert()
        .success();
    let scaffold_value = parse_json(&scaffold.get_output().stdout);
    assert_eq!(scaffold_value["schema-version"], 2);
    assert_eq!(scaffold_value["target"], "core");
    assert_eq!(scaffold_value["app-name"], "Counter");
    assert!(fixture.project.join("shared/src/app.rs").is_file());
    assert!(!fixture.project.join("iOS").exists(), "core scaffold must stay render-only");
    assert!(!fixture.project.join("Android").exists(), "core scaffold must stay render-only");

    let overwrite = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .args(["tool", "run", "vectis-scaffold", "--", "core", "Counter"])
        .assert()
        .failure();
    assert_eq!(overwrite.get_output().status.code(), Some(1));
    let overwrite_value = parse_json(&overwrite.get_output().stdout);
    assert_eq!(overwrite_value["schema-version"], 2);
    assert_eq!(overwrite_value["error"], "invalid-project");
    assert!(
        overwrite_value["message"]
            .as_str()
            .expect("overwrite message")
            .contains("refusing to overwrite existing file"),
        "{overwrite_value}"
    );

    let denied = VectisToolFixture::with_scaffold_write_outside_project();
    let denied_value = specify()
        .current_dir(&denied.project)
        .env("SPECIFY_TOOLS_CACHE", &denied.cache)
        .args(["--format", "json", "tool", "run", "vectis-scaffold", "--", "core", "Counter"])
        .assert()
        .failure();
    assert_eq!(denied_value.get_output().status.code(), Some(2));
    let denied_json = parse_json(&denied_value.get_output().stdout);
    assert_eq!(denied_json["error"], "tool-permission-denied", "{denied_json}");
    assert!(
        denied_json["message"]
            .as_str()
            .expect("denied message")
            .contains("escapes PROJECT_DIR/CAPABILITY_DIR"),
        "{denied_json}"
    );
}

#[test]
fn vectis_tools_run_through_fetch_cache_permissions_and_exit_codes() {
    let fixture = VectisToolFixture::with_project_write();
    let clean_tokens = fixture.write_tokens("tokens.yaml", "version: 1\n");
    let broken_tokens = fixture.write_tokens(
        "broken-tokens.yaml",
        "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n",
    );

    let list = run_json(&fixture.project, &fixture.cache, &["tool", "list"]);
    let tools = list["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["vectis-validate", "vectis-scaffold"], "{list}");
    assert!(tools.iter().all(|tool| tool["scope"] == "capability"), "{list}");
    assert!(tools.iter().all(|tool| tool["scope-detail"] == "vectis"), "{list}");
    assert!(tools.iter().all(|tool| tool["cache-status"] == "miss-not-found"), "{list}");

    let fetch = run_json(&fixture.project, &fixture.cache, &["tool", "fetch"]);
    let fetched = fetch["tools"].as_array().expect("fetched tools array");
    assert_eq!(fetched.len(), 2, "{fetch}");
    assert!(fetched.iter().all(|tool| tool["fetched"] == true), "{fetch}");

    let show = run_json(&fixture.project, &fixture.cache, &["tool", "show", "vectis-scaffold"]);
    assert_eq!(show["tool"]["name"], "vectis-scaffold");
    assert_eq!(show["tool"]["cache-status"], "hit");
    assert_eq!(show["tool"]["permissions"]["write"], serde_json::json!(["$PROJECT_DIR"]));
    assert!(show["tool"]["sha256"].as_str().is_some_and(|sha| sha.len() == 64), "{show}");
    assert!(show["tool"]["fetched-at"].as_str().is_some_and(|value| !value.is_empty()), "{show}");

    let clean = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .arg("tool")
        .arg("run")
        .arg("vectis-validate")
        .arg("--")
        .arg("tokens")
        .arg(&clean_tokens)
        .assert()
        .success();
    let clean_value = parse_json(&clean.get_output().stdout);
    assert_eq!(clean_value["schema-version"], 2);
    assert_eq!(clean_value["mode"], "tokens");
    assert_eq!(clean_value["errors"].as_array().map(Vec::len), Some(0), "{clean_value}");

    let findings = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .arg("tool")
        .arg("run")
        .arg("vectis-validate")
        .arg("--")
        .arg("tokens")
        .arg(&broken_tokens)
        .assert()
        .failure();
    assert_eq!(findings.get_output().status.code(), Some(1));
    let findings_value = parse_json(&findings.get_output().stdout);
    assert_eq!(findings_value["schema-version"], 2);
    assert_eq!(findings_value["mode"], "tokens");
    assert_eq!(findings_value["errors"].as_array().map(Vec::len), Some(1), "{findings_value}");

    assert_scaffold_run_and_permission_denial(&fixture);
}
