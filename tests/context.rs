//! Integration harness for `specify context`.
//!
//! These tests cover the context acceptance matrix with structural assertions so
//! generated prose can evolve without brittle snapshots.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use jsonschema::Validator;
use serde_json::Value as JsonValue;
use tempfile::{TempDir, tempdir};

const CONTEXT_LOCK_EXAMPLE: &str = r"
version: 1
fingerprint: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
cli_version: 0.2.0
inputs:
  - path: .specify/project.yaml
    sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  - path: registry.yaml
    sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
fences:
  body_sha256: sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd
";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn omnia_schema_dir() -> PathBuf {
    repo_root().join("schemas").join("omnia")
}

fn context_lock_schema_path() -> PathBuf {
    repo_root().join("schemas").join("context-lock.schema.json")
}

fn load_context_lock_validator() -> Validator {
    let raw =
        fs::read_to_string(context_lock_schema_path()).expect("read context-lock.schema.json");
    let schema: JsonValue =
        serde_json::from_str(&raw).expect("context-lock.schema.json is valid JSON");
    jsonschema::validator_for(&schema).expect("context-lock.schema.json compiles")
}

fn yaml_to_json(yaml: &str) -> JsonValue {
    serde_saphyr::from_str(yaml).expect("fixture parses as YAML")
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

struct ContextProject {
    _tmp: TempDir,
    root: PathBuf,
}

impl ContextProject {
    fn new() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        Self { _tmp: tmp, root }
    }

    fn initialized() -> Self {
        let project = Self::new();
        project.init().success();
        project
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn command(&self) -> Command {
        let mut cmd = specify();
        cmd.current_dir(self.path());
        cmd
    }

    fn init(&self) -> Assert {
        let mut cmd = self.command();
        cmd.args(["init"]).arg(omnia_schema_dir()).args(["--name", "context-demo"]).assert()
    }

    fn init_hub(&self) -> Assert {
        let mut cmd = self.command();
        cmd.args(["init", "--hub", "--name", "context-hub"]).assert()
    }

    fn agents_path(&self) -> PathBuf {
        self.path().join("AGENTS.md")
    }

    fn lock_path(&self) -> PathBuf {
        self.path().join(".specify/context.lock")
    }

    fn read_agents(&self) -> String {
        fs::read_to_string(self.agents_path()).expect("read AGENTS.md")
    }

    fn read_lock(&self) -> String {
        fs::read_to_string(self.lock_path()).expect("read .specify/context.lock")
    }

    fn write_agents(&self, contents: &str) {
        fs::write(self.agents_path(), contents).expect("write AGENTS.md");
    }

    fn write_lock(&self, contents: &str) {
        fs::write(self.lock_path(), contents).expect("write .specify/context.lock");
    }

    fn write_registry(&self, contents: &str) {
        fs::write(self.path().join("registry.yaml"), contents).expect("write registry.yaml");
    }

    fn remove_agents(&self) {
        fs::remove_file(self.agents_path()).expect("remove AGENTS.md");
    }

    fn remove_lock(&self) {
        fs::remove_file(self.lock_path()).expect("remove .specify/context.lock");
    }

    fn context_generate(&self, args: &[&str]) -> Assert {
        let mut cmd = self.command();
        cmd.args(["context", "generate"]).args(args).assert()
    }

    fn context_check(&self, args: &[&str]) -> Assert {
        let mut cmd = self.command();
        cmd.args(["context", "check"]).args(args).assert()
    }

    fn workspace_sync(&self) -> Assert {
        let mut cmd = self.command();
        cmd.args(["workspace", "sync"]).assert()
    }
}

fn assert_context_fences(contents: &str) {
    assert!(
        contents.contains("<!-- specify:context begin"),
        "AGENTS.md must contain the opening context fence, got:\n{contents}"
    );
    assert!(
        contents.contains("<!-- specify:context end -->"),
        "AGENTS.md must contain the closing context fence, got:\n{contents}"
    );
}

fn assert_stdout_contains(assert: &Assert, expected: &str) {
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains(expected), "stdout must contain `{expected}`, got:\n{stdout}");
}

fn assert_stderr_contains(assert: &Assert, expected: &str) {
    let output = assert.get_output();
    let stderr = String::from_utf8(output.stderr.clone()).expect("utf8 stderr");
    assert!(stderr.contains(expected), "stderr must contain `{expected}`, got:\n{stderr}");
}

fn stdout_json(assert: &Assert) -> JsonValue {
    serde_json::from_slice(&assert.get_output().stdout).expect("stdout json")
}

fn assert_context_lock_valid(project: &ContextProject) {
    let validator = load_context_lock_validator();
    let instance = yaml_to_json(&project.read_lock());
    let errors: Vec<String> =
        validator.iter_errors(&instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();

    assert!(errors.is_empty(), "context.lock should validate; errors: {errors:#?}");
}

fn section_bullets<'a>(contents: &'a str, heading: &str, next_heading: &str) -> Vec<&'a str> {
    let section = contents
        .split_once(heading)
        .and_then(|(_before, after)| after.split_once(next_heading).map(|(section, _rest)| section))
        .expect("section bounded by headings");
    section.lines().filter_map(|line| line.strip_prefix("- ")).collect()
}

#[test]
fn context_lock_schema_validates_yaml_shape() {
    let validator = load_context_lock_validator();
    let instance = yaml_to_json(CONTEXT_LOCK_EXAMPLE);
    let errors: Vec<String> =
        validator.iter_errors(&instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();

    assert!(errors.is_empty(), "context.lock example should validate; errors: {errors:#?}");
}

#[test]
fn context_lock_schema_rejects_json_output_key_names() {
    let validator = load_context_lock_validator();
    let output_style = CONTEXT_LOCK_EXAMPLE
        .replace("cli_version", "cli-version")
        .replace("body_sha256", "body-sha256");
    let instance = yaml_to_json(&output_style);
    let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();

    assert!(!errors.is_empty(), "lock schema must require serialized YAML snake_case keys");
}

#[test]
fn init_regular_project_creates_agents_md() {
    let project = ContextProject::initialized();

    let agents = project.read_agents();
    assert_context_fences(&agents);
    assert!(agents.contains("# context-demo - Agent Instructions"));
    for heading in [
        "## Runtime",
        "## Tests",
        "## Linting",
        "## Navigation",
        "## Conventions",
        "## Boundaries",
        "## Dependencies",
    ] {
        assert!(agents.contains(heading), "AGENTS.md must contain `{heading}`, got:\n{agents}");
    }
    assert_context_lock_valid(&project);
}

#[test]
fn init_hub_project_creates_hub_shaped_agents_md() {
    let project = ContextProject::new();
    project.init_hub().success();

    let mut agents = project.read_agents();
    assert_context_fences(&agents);
    assert!(agents.contains("# context-hub - Agent Instructions"));
    for heading in ["## Navigation", "## Conventions", "## Boundaries", "## Dependencies"] {
        assert!(agents.contains(heading), "hub AGENTS.md must contain `{heading}`, got:\n{agents}");
    }
    for heading in ["## Runtime", "## Tests", "## Linting"] {
        assert!(!agents.contains(heading), "hub AGENTS.md must omit `{heading}`, got:\n{agents}");
    }
    assert_context_lock_valid(&project);

    project.remove_agents();
    project.context_generate(&[]).success();
    agents = project.read_agents();
    assert_context_fences(&agents);
    for heading in ["## Runtime", "## Tests", "## Linting"] {
        assert!(!agents.contains(heading), "hub regenerate must omit `{heading}`, got:\n{agents}");
    }
}

#[test]
fn init_preserves_pre_existing_agents_md_byte_for_byte() {
    let project = ContextProject::new();
    let hand_authored = "# Hand Authored\n\nKeep this exact file.\n";
    project.write_agents(hand_authored);

    let init = project.init().success();

    assert_stdout_contains(&init, "AGENTS.md already present; skipping context generate");
    assert_eq!(project.read_agents(), hand_authored);
}

#[test]
fn context_generate_writes_agents_md_when_missing() {
    let project = ContextProject::initialized();
    project.remove_agents();

    let generate = project.context_generate(&[]).success();
    assert_stdout_contains(&generate, "wrote AGENTS.md");

    let agents = project.read_agents();
    assert_context_fences(&agents);
    assert!(agents.contains("# context-demo - Agent Instructions"));
    for heading in [
        "## Runtime",
        "## Tests",
        "## Linting",
        "## Navigation",
        "## Conventions",
        "## Boundaries",
        "## Dependencies",
    ] {
        assert!(agents.contains(heading), "AGENTS.md must contain `{heading}`, got:\n{agents}");
    }
    assert_context_lock_valid(&project);
}

#[test]
fn context_generate_check_succeeds_when_clean() {
    let project = ContextProject::initialized();
    let before = project.read_agents();

    let check = project.context_generate(&["--check"]).success();

    assert_stdout_contains(&check, "AGENTS.md is up to date");
    assert_eq!(project.read_agents(), before, "--check must not rewrite a clean file");
}

#[test]
fn context_generate_check_fails_when_generation_would_update_file() {
    let project = ContextProject::initialized();
    let stale =
        project.read_agents().replace("## Runtime\n- not detected\n", "## Runtime\n- stale\n");
    project.write_agents(&stale);

    let check = project.context_generate(&["--check"]).code(1);

    assert_stdout_contains(&check, "context is out of date");
    assert_eq!(project.read_agents(), stale, "--check must not write planned changes");
}

#[test]
fn context_generate_check_fails_when_agents_md_is_missing() {
    let project = ContextProject::initialized();
    project.remove_agents();

    let check = project.context_generate(&["--check"]).code(1);

    assert_stdout_contains(&check, "context is out of date");
    assert!(!project.agents_path().exists(), "--check must not create AGENTS.md");
}

#[test]
fn context_generate_json_uses_existing_envelope_and_kebab_case_keys() {
    let project = ContextProject::initialized();
    project.remove_agents();
    let mut cmd = project.command();
    let assert = cmd.args(["--format", "json", "context", "generate"]).assert().success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("stdout json");

    assert_eq!(value["schema-version"], 4);
    assert_eq!(value["status"], "written");
    assert_eq!(value["path"], "AGENTS.md");
    assert_eq!(value["check"], false);
    assert_eq!(value["force"], false);
    assert_eq!(value["changed"], true);
    assert_eq!(value["disposition"], "create");
    assert!(value.get("schema_version").is_none(), "JSON keys must stay kebab-case");
}

#[test]
fn context_generate_reports_unfenced_agents_md_with_stable_error() {
    let project = ContextProject::initialized();
    project.write_agents("# Hand Authored\n\nKeep me.\n");

    let mut cmd = project.command();
    let assert = cmd.args(["--format", "json", "context", "generate"]).assert().code(1);
    let value = stdout_json(&assert);

    assert_eq!(value["error"], "context-existing-unfenced-agents-md");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(project.read_agents(), "# Hand Authored\n\nKeep me.\n");

    project.context_generate(&["--force"]).success();
    let agents = project.read_agents();
    assert_context_fences(&agents);
    assert!(
        !agents.contains("Keep me."),
        "--force must replace the unfenced hand-authored file:\n{agents}"
    );
    assert_context_lock_valid(&project);
    project.context_check(&[]).success();
}

#[test]
fn context_hub_dependencies_include_peer_descriptions() {
    let project = ContextProject::new();
    project.init_hub().success();
    project.write_registry(
        "\
version: 1
projects:
  - name: billing
    url: billing-src
    capability: omnia@v1
    description: Billing service
  - name: orders
    url: orders-src
    capability: omnia@v1
    description: Orders service
",
    );

    project.context_generate(&[]).success();
    let agents = project.read_agents();

    assert!(!agents.contains("## Runtime"), "hub AGENTS.md must permanently omit Runtime");
    assert!(!agents.contains("## Tests"), "hub AGENTS.md must permanently omit Tests");
    assert!(!agents.contains("## Linting"), "hub AGENTS.md must permanently omit Linting");
    assert!(
        agents.contains("`billing` @ `omnia@v1` -> `billing-src`. Description: Billing service."),
        "billing dependency must include description:\n{agents}"
    );
    assert!(
        agents.contains("`orders` @ `omnia@v1` -> `orders-src`. Description: Orders service."),
        "orders dependency must include description:\n{agents}"
    );
}

#[test]
fn context_navigation_lists_synced_workspace_clones() {
    let project = ContextProject::new();
    project.init_hub().success();
    fs::create_dir_all(project.path().join("billing-src")).expect("billing peer dir");
    fs::create_dir_all(project.path().join("orders-src")).expect("orders peer dir");
    project.write_registry(
        "\
version: 1
projects:
  - name: billing
    url: billing-src
    capability: omnia@v1
    description: Billing service
  - name: orders
    url: orders-src
    capability: omnia@v1
    description: Orders service
",
    );
    project.workspace_sync().success();

    project.context_generate(&[]).success();
    let agents = project.read_agents();

    assert!(
        agents.contains(
            "`.specify/workspace/billing/` is the materialized workspace clone for registry peer `billing`."
        ),
        "billing workspace path must appear repo-relative under Navigation:\n{agents}"
    );
    assert!(
        agents.contains(
            "`.specify/workspace/orders/` is the materialized workspace clone for registry peer `orders`."
        ),
        "orders workspace path must appear repo-relative under Navigation:\n{agents}"
    );
}

#[test]
fn context_single_repo_project_renders_no_registered_peers() {
    let project = ContextProject::initialized();

    let agents = project.read_agents();

    assert!(
        agents.contains("single-repo project; no registered peers."),
        "single-repo projects must render explicit dependency fallback:\n{agents}"
    );
}

#[test]
fn context_detects_cargo_runtime_tests_and_clippy() {
    let project = ContextProject::initialized();
    fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n")
        .expect("write Cargo.toml");
    fs::write(project.path().join("rust-toolchain.toml"), "[toolchain]\nchannel = \"stable\"\n")
        .expect("write rust-toolchain.toml");
    fs::write(project.path().join("clippy.toml"), "avoid-breaking-exported-api = false\n")
        .expect("write clippy.toml");

    project.context_generate(&[]).success();

    let agents = project.read_agents();
    assert!(agents.contains("detected: Rust (toolchain `stable`)."), "{agents}");
    assert!(agents.contains("detected: `cargo test`."), "{agents}");
    assert!(agents.contains("detected: `cargo clippy`."), "{agents}");
}

#[test]
fn context_detects_npm_runtime_tests_and_lint_script() {
    let project = ContextProject::initialized();
    fs::write(
        project.path().join("package.json"),
        r#"{
          "engines": { "node": ">=20" },
          "scripts": { "test": "vitest run", "lint": "eslint ." }
        }"#,
    )
    .expect("write package.json");

    project.context_generate(&[]).success();

    let agents = project.read_agents();
    assert!(agents.contains("detected: Node.js (engines.node `>=20`)."), "{agents}");
    assert!(agents.contains("detected: `npm test`."), "{agents}");
    assert!(agents.contains("detected: `npm run lint`."), "{agents}");
}

#[test]
fn context_orders_mixed_runtime_bullets_deterministically() {
    let project = ContextProject::initialized();
    fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n")
        .expect("write Cargo.toml");
    fs::write(project.path().join("package.json"), r#"{"engines":{"node":">=20"}}"#)
        .expect("write package.json");
    fs::write(project.path().join("pyproject.toml"), "[project]\nname = \"demo\"\n")
        .expect("write pyproject.toml");
    fs::write(project.path().join("go.mod"), "module demo\n\ngo 1.22\n").expect("write go.mod");

    project.context_generate(&[]).success();

    let agents = project.read_agents();
    let runtime_bullets = section_bullets(&agents, "## Runtime\n", "\n## Tests");
    assert_eq!(
        runtime_bullets,
        vec![
            "detected: Go 1.22.",
            "detected: Node.js (engines.node `>=20`).",
            "detected: Python (pyproject.toml).",
            "detected: Rust.",
        ]
    );
}

#[test]
fn context_warns_for_corrupt_markers_and_renders_not_detected() {
    let project = ContextProject::initialized();
    fs::write(project.path().join("Cargo.toml"), "package = [").expect("write Cargo.toml");
    fs::write(project.path().join("package.json"), "{").expect("write package.json");
    fs::create_dir_all(project.path().join(".github/workflows")).expect("create workflows");
    fs::write(project.path().join(".github/workflows/ci.yaml"), "name: [unterminated\n")
        .expect("write workflow");

    let generate = project.context_generate(&[]).success();

    assert_stderr_contains(&generate, "warning: .github/workflows/ci.yaml:");
    assert_stderr_contains(&generate, "warning: Cargo.toml:");
    assert_stderr_contains(&generate, "warning: package.json:");
    let agents = project.read_agents();
    assert!(agents.contains("## Runtime\n- not detected\n"), "{agents}");
    assert!(agents.contains("## Tests\n- not detected\n"), "{agents}");
    assert!(agents.contains("## Linting\n- not detected\n"), "{agents}");
}

#[test]
fn context_generate_refuses_inside_workspace_clone() {
    let hub = ContextProject::initialized();
    let clone_root = hub.path().join(".specify/workspace/peer");
    fs::create_dir_all(&clone_root).expect("create workspace clone");
    specify()
        .current_dir(&clone_root)
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "peer"])
        .assert()
        .success();
    assert!(
        !clone_root.join("AGENTS.md").exists(),
        "init inside a workspace clone must not generate nested AGENTS.md"
    );

    specify().current_dir(&clone_root).args(["context", "generate"]).assert().failure();
}

#[test]
fn context_harness_reads_agents_and_asserts_fences() {
    let project = ContextProject::new();
    project.write_agents(
        "# context-demo - Agent Instructions\n\n\
         <!-- specify:context begin\n\
         fingerprint: sha256:test\n\
         -->\n\n\
         ## Runtime\n\
         - not detected\n\n\
         <!-- specify:context end -->\n",
    );

    let agents = project.read_agents();
    assert_context_fences(&agents);
}

#[test]
fn context_check_is_green_after_repeated_generate() {
    let project = ContextProject::initialized();
    let initial_agents = project.read_agents();
    let initial_lock = project.read_lock();

    project.context_generate(&[]).success();
    project.context_generate(&[]).success();
    let check = project.context_check(&[]).success();

    assert_stdout_contains(&check, "context up to date");
    assert_eq!(
        project.read_agents(),
        initial_agents,
        "clean generate must be byte-identical for AGENTS.md"
    );
    assert_eq!(project.read_lock(), initial_lock, "clean generate must not rewrite context.lock");
}

#[test]
fn context_check_reports_registry_input_drift() {
    let project = ContextProject::initialized();
    project.write_registry(
        "\
version: 1
projects:
  - name: context-demo
    url: .
    capability: omnia@v1
    description: Context demo
",
    );
    project.context_generate(&[]).success();

    project.write_registry(
        "\
version: 1
projects:
  - name: context-demo
    url: .
    capability: omnia@v1
    description: Context demo
  - name: billing
    url: ../billing
    capability: omnia@v1
    description: Billing service
",
    );

    let check = project.context_check(&["--format", "json"]).code(1);
    let value = stdout_json(&check);

    assert_eq!(value["status"], "drift");
    assert_eq!(value["inputs-changed"], serde_json::json!(["registry.yaml"]));
    assert_eq!(value["inputs-added"], serde_json::json!([]));
    assert_eq!(value["inputs-removed"], serde_json::json!([]));
    assert_eq!(value["fences-modified"], false);
}

#[test]
fn context_check_reports_fenced_body_drift() {
    let project = ContextProject::initialized();
    let edited =
        project.read_agents().replace("## Runtime\n- not detected\n", "## Runtime\n- edited\n");
    project.write_agents(&edited);

    let check = project.context_check(&["--format", "json"]).code(1);
    let value = stdout_json(&check);

    assert_eq!(value["status"], "drift");
    assert_eq!(value["inputs-changed"], serde_json::json!([]));
    assert_eq!(value["inputs-added"], serde_json::json!([]));
    assert_eq!(value["inputs-removed"], serde_json::json!([]));
    assert_eq!(value["fences-modified"], true);
}

#[test]
fn context_check_distinguishes_missing_agents_and_missing_lock() {
    let missing_agents = ContextProject::initialized();
    missing_agents.remove_agents();

    let check = missing_agents.context_check(&["--format", "json"]).code(1);
    let value = stdout_json(&check);
    assert_eq!(value["status"], "context-not-generated");

    let missing_lock = ContextProject::initialized();
    missing_lock.remove_lock();

    let check = missing_lock.context_check(&["--format", "json"]).code(1);
    let value = stdout_json(&check);
    assert_eq!(value["status"], "context-lock-missing");
}

#[test]
fn context_check_rejects_newer_lock_version_as_validation_error() {
    let project = ContextProject::initialized();
    let newer = project.read_lock().replacen("version: 1", "version: 999", 1);
    project.write_lock(&newer);

    let check = project.context_check(&["--format", "json"]).code(2);
    let value = stdout_json(&check);

    assert_eq!(value["error"], "validation");
    assert_eq!(value["exit-code"], 2);
    assert_eq!(value["results"][0]["rule-id"], "context-lock-version-too-new");
    assert!(
        value["results"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("context-lock-version-too-new:")),
        "newer lock diagnostic detail should start with context-lock-version-too-new, got:\n{value}"
    );
}

#[test]
fn context_check_rejects_malformed_lock_with_stable_validation_rule() {
    let project = ContextProject::initialized();
    project.write_lock("version: one\n");

    let check = project.context_check(&["--format", "json"]).code(2);
    let value = stdout_json(&check);

    assert_eq!(value["error"], "validation");
    assert_eq!(value["exit-code"], 2);
    assert_eq!(value["results"][0]["rule-id"], "context-lock-malformed");
    assert!(
        value["results"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("context-lock-malformed:")),
        "malformed lock diagnostic detail should start with context-lock-malformed, got:\n{value}"
    );
}

#[test]
fn context_generate_refuses_to_overwrite_modified_fenced_content_without_force() {
    let project = ContextProject::initialized();
    let edited =
        project.read_agents().replace("## Runtime\n- not detected\n", "## Runtime\n- edited\n");
    project.write_agents(&edited);

    let mut cmd = project.command();
    let generate = cmd.args(["--format", "json", "context", "generate"]).assert().code(1);
    let value = stdout_json(&generate);

    assert_eq!(value["error"], "context-fenced-content-modified");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(project.read_agents(), edited, "failed generate must preserve edited fenced body");

    project.context_generate(&["--force"]).success();
    project.context_check(&[]).success();
}
