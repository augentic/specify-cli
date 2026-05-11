//! Integration coverage for the Vectis WASI tool declared through `specify tool`.

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

fn vectis_wasi_artifact() -> &'static PathBuf {
    static ARTIFACT: OnceLock<PathBuf> = OnceLock::new();
    ARTIFACT.get_or_init(|| {
        let root = repo_root();
        let dist = root.join("target/vectis-wasi-tools/release");
        let wasm = dist.join("vectis.wasm");
        if !wasm.is_file() {
            build_vectis_wasi_artifact(&root, &dist, &wasm);
        }
        assert!(wasm.is_file(), "missing Vectis WASI artifact at {}", wasm.display());
        wasm
    })
}

fn build_vectis_wasi_artifact(root: &Path, dist: &Path, wasm: &Path) {
    let wasi_workspace = root.join("wasi-tools");
    let status = ProcessCommand::new("cargo")
        .current_dir(&wasi_workspace)
        .args([
            "build",
            "-p",
            "specify-vectis",
            "--bin",
            "vectis",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--locked",
        ])
        .status()
        .unwrap_or_else(|err| panic!("failed to invoke cargo for Vectis WASI artifact: {err}"));
    assert!(
        status.success(),
        "failed to build Vectis WASI artifact with `cargo build -p specify-vectis --bin vectis --target wasm32-wasip2 --release --locked`"
    );

    fs::create_dir_all(dist).expect("create Vectis WASI dist dir");
    fs::copy(wasi_workspace.join("target/wasm32-wasip2/release/vectis.wasm"), wasm)
        .expect("copy vectis.wasm to dist");
}

struct VectisToolFixture {
    _tmp: TempDir,
    project: PathBuf,
    cache: PathBuf,
}

impl VectisToolFixture {
    fn from_tempdir(tmp: TempDir, write_permission: &str) -> Self {
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

        let artifact = vectis_wasi_artifact();
        let source = file_uri(artifact);
        let sha = sha256_hex(artifact);
        fs::write(
            capability.join("tools.yaml"),
            format!(
                "tools:\n  - name: vectis\n    version: 0.2.0\n    source: \"{source}\"\n    sha256: \"{sha}\"\n    permissions:\n      read:\n        - \"$PROJECT_DIR\"\n        - \"$CAPABILITY_DIR\"\n      write:\n        - \"{write_permission}\"\n"
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
        .args(["tool", "run", "vectis", "--", "scaffold", "core", "Counter"])
        .assert()
        .success();
    let scaffold_value = parse_json(&scaffold.get_output().stdout);
    assert_eq!(scaffold_value["envelope-version"], 2);
    assert_eq!(scaffold_value["target"], "core");
    assert_eq!(scaffold_value["app-name"], "Counter");
    assert!(fixture.project.join("shared/src/app.rs").is_file());
    assert!(!fixture.project.join("iOS").exists(), "core scaffold must stay render-only");
    assert!(!fixture.project.join("Android").exists(), "core scaffold must stay render-only");

    let overwrite = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .args(["tool", "run", "vectis", "--", "scaffold", "core", "Counter"])
        .assert()
        .failure();
    assert_eq!(overwrite.get_output().status.code(), Some(1));
    let overwrite_value = parse_json(&overwrite.get_output().stdout);
    assert_eq!(overwrite_value["envelope-version"], 2);
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
        .args(["--format", "json", "tool", "run", "vectis", "--", "scaffold", "core", "Counter"])
        .assert()
        .failure();
    assert_eq!(denied_value.get_output().status.code(), Some(2));
    let denied_json = parse_json(&denied_value.get_output().stderr);
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
fn runs_through_fetch_cache_perms_and_exits() {
    let fixture = VectisToolFixture::with_project_write();
    let clean_tokens = fixture.write_tokens("tokens.yaml", "version: 1\n");
    let broken_tokens = fixture.write_tokens(
        "broken-tokens.yaml",
        "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n",
    );

    let list = run_json(&fixture.project, &fixture.cache, &["tool", "list"]);
    let tools = list["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["vectis"], "{list}");
    assert!(tools.iter().all(|tool| tool["scope"] == "capability"), "{list}");
    assert!(tools.iter().all(|tool| tool["scope-detail"] == "vectis"), "{list}");
    assert!(tools.iter().all(|tool| tool["cache-status"] == "miss-not-found"), "{list}");

    let fetch = run_json(&fixture.project, &fixture.cache, &["tool", "fetch"]);
    let fetched = fetch["tools"].as_array().expect("fetched tools array");
    assert_eq!(fetched.len(), 1, "{fetch}");
    assert!(fetched.iter().all(|tool| tool["fetched"] == true), "{fetch}");

    let show = run_json(&fixture.project, &fixture.cache, &["tool", "show", "vectis"]);
    assert_eq!(show["tool"]["name"], "vectis");
    assert_eq!(show["tool"]["cache-status"], "hit");
    assert_eq!(show["tool"]["permissions"]["write"], serde_json::json!(["$PROJECT_DIR"]));
    assert!(show["tool"]["sha256"].as_str().is_some_and(|sha| sha.len() == 64), "{show}");
    assert!(show["tool"]["fetched-at"].as_str().is_some_and(|value| !value.is_empty()), "{show}");

    let clean = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .arg("tool")
        .arg("run")
        .arg("vectis")
        .arg("--")
        .arg("validate")
        .arg("tokens")
        .arg(&clean_tokens)
        .assert()
        .success();
    let clean_value = parse_json(&clean.get_output().stdout);
    assert_eq!(clean_value["envelope-version"], 2);
    assert_eq!(clean_value["mode"], "tokens");
    assert_eq!(clean_value["errors"].as_array().map(Vec::len), Some(0), "{clean_value}");

    let findings = specify()
        .current_dir(&fixture.project)
        .env("SPECIFY_TOOLS_CACHE", &fixture.cache)
        .arg("tool")
        .arg("run")
        .arg("vectis")
        .arg("--")
        .arg("validate")
        .arg("tokens")
        .arg(&broken_tokens)
        .assert()
        .failure();
    assert_eq!(findings.get_output().status.code(), Some(1));
    let findings_value = parse_json(&findings.get_output().stdout);
    assert_eq!(findings_value["envelope-version"], 2);
    assert_eq!(findings_value["mode"], "tokens");
    assert_eq!(findings_value["errors"].as_array().map(Vec::len), Some(1), "{findings_value}");

    assert_scaffold_run_and_permission_denial(&fixture);
}
