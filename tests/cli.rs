//! Integration tests for the `specify` CLI binary.
//!
//! Each test spawns the built binary via `assert_cmd::Command::cargo_bin`
//! from a fresh `tempfile::TempDir`, so stdout/stderr and filesystem side
//! effects are observed exactly as a user would experience them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use tempfile::tempdir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn omnia_schema_dir() -> PathBuf {
    repo_root().join("schemas").join("omnia")
}

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

const GIT_TEST_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Specify Test"),
    ("GIT_AUTHOR_EMAIL", "specify-test@example.com"),
    ("GIT_COMMITTER_NAME", "Specify Test"),
    ("GIT_COMMITTER_EMAIL", "specify-test@example.com"),
];

fn run_git(root: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .current_dir(root)
        .args(args)
        .envs(GIT_TEST_ENV)
        .output()
        .unwrap_or_else(|err| panic!("git {} failed to start: {err}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout utf8")
}

#[test]
fn help_exits_zero_and_prints_usage() {
    let assert = specify().arg("--help").assert().success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        output.contains("specify") && output.contains("Usage"),
        "expected usage in stdout, got:\n{output}"
    );
}

#[test]
fn init_text_format_succeeds() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("Initialized"));
    assert!(stdout.contains("omnia"));
    assert!(stdout.contains(".specify/project.yaml"));

    let config_path = tmp.path().join(".specify/project.yaml");
    assert!(config_path.is_file(), "project.yaml must exist");
}

#[test]
fn init_json_format_has_stable_shape() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["schema-name"], "omnia");
    assert!(value["config-path"].is_string());
    let config_path = value["config-path"].as_str().unwrap();
    // Canonicalized tmp path so substring match handles macOS
    // /private/var symlinks gracefully.
    let canonical_tmp = fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    assert!(
        config_path.starts_with(canonical_tmp.to_string_lossy().as_ref()),
        "config_path {config_path} should start with {}",
        canonical_tmp.display()
    );
    assert!(value["specify-version"].is_string());
    assert!(value["scaffolded-rule-keys"].is_array());
}

#[test]
fn init_rejects_removed_schema_dir_syntax() {
    let tmp = tempdir().unwrap();
    specify()
        .current_dir(tmp.path())
        .args(["init", "omnia", "--schema-dir"])
        .arg(repo_root())
        .assert()
        .failure();
}

#[test]
#[ignore = "networked GitHub fetch smoke test"]
fn init_github_directory_uri_succeeds() {
    let tmp = tempdir().unwrap();
    specify()
        .current_dir(tmp.path())
        .args(["init", "https://github.com/augentic/specify/capabilities/omnia", "--name", "demo"])
        .assert()
        .success();
}

// ---- RFC-13 Phase 1.3: positional <capability> + --hub mutual exclusion ----

#[test]
fn init_writes_capability_field_for_url_arg() {
    // Acceptance (a): `specify init <url>` writes `capability: <url>`
    // and no `schema:` field; `hub:` either absent or false.
    let tmp = tempdir().unwrap();
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let project_yaml =
        fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read project.yaml");
    assert!(
        project_yaml.contains("capability:"),
        "project.yaml must carry `capability:` after non-hub init, got:\n{project_yaml}"
    );
    assert!(
        !project_yaml.lines().any(|line| line.trim_start().starts_with("schema:")),
        "project.yaml must NOT carry the legacy `schema:` field, got:\n{project_yaml}"
    );
    // hub: absent (or false) means the value is implicit; just check no
    // `hub: true` line.
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("hub: true")),
        "non-hub init must not write `hub: true`, got:\n{project_yaml}"
    );

    // RFC-13 chunk 2.9 — non-hub init writes only `project.yaml` and
    // the `.specify/` skeleton at the project root. Platform-component
    // artefacts at the repo root are operator-managed: `specify
    // registry add` mints `registry.yaml`, `specify change create`
    // mints `change.md` (post-Phase-3.7; the legacy filename was
    // `initiative.md`), and `specify change plan create` mints
    // `plan.yaml`. Init must not pre-touch any of them.
    for absent in ["registry.yaml", "initiative.md", "plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "non-hub init must not pre-touch `{absent}` at the repo root"
        );
    }
}

#[test]
fn init_with_no_args_errors_with_init_requires_capability_or_hub() {
    // Acceptance (c): `specify init` (no positional, no `--hub`) must
    // exit non-zero with the `init-requires-capability-or-hub`
    // diagnostic.
    let tmp = tempdir().unwrap();
    let assert = specify().current_dir(tmp.path()).args(["init"]).assert().failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_ne!(code, 0, "init with no args must exit non-zero");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("init-requires-capability-or-hub"),
        "diagnostic must carry the stable code, got stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !tmp.path().join(".specify").exists(),
        "no .specify must be scaffolded on validation failure"
    );
}

#[test]
fn init_json_with_no_args_errors_with_stable_code() {
    let tmp = tempdir().unwrap();
    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "init"]).assert().failure();

    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("stdout is JSON");
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["error"], "init-requires-capability-or-hub");
    assert_eq!(value["exit-code"], 1);
    let message = value["message"].as_str().expect("message string");
    assert!(message.contains("specify init <capability>"), "message: {message}");
    assert!(message.contains("specify init --hub"), "message: {message}");
    assert!(
        !tmp.path().join(".specify").exists(),
        "no .specify must be scaffolded on validation failure"
    );
}

#[test]
fn init_with_capability_and_hub_errors_with_init_requires_capability_or_hub() {
    // Acceptance (d): `specify init <url> --hub` must exit non-zero
    // with the same diagnostic.
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .arg("--hub")
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_ne!(code, 0, "init with both capability and --hub must exit non-zero");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("init-requires-capability-or-hub"),
        "diagnostic must carry the stable code, got stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn init_help_no_longer_advertises_schema_uri_flag() {
    // RFC-13 §1.3: `--schema-uri` is gone from the post-Phase-1 surface.
    let assert = specify().args(["init", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(
        !stdout.contains("--schema-uri"),
        "post-RFC-13 init --help must not advertise `--schema-uri`, got:\n{stdout}"
    );
    assert!(
        stdout.contains("CAPABILITY") || stdout.contains("capability"),
        "init --help must mention the capability positional, got:\n{stdout}"
    );
    assert!(stdout.contains("--hub"), "init --help must still document --hub, got:\n{stdout}");
}

#[test]
fn project_aware_command_refuses_legacy_schema_field_with_schema_became_capability() {
    // Acceptance (e): a v1 `project.yaml` carrying `schema:` must be
    // rejected loudly when a project-aware verb tries to load it.
    // `specify status` is the canonical project-aware entry point and
    // loads `ProjectConfig` first thing.
    let tmp = tempdir().unwrap();
    let specify_dir = tmp.path().join(".specify");
    fs::create_dir_all(&specify_dir).unwrap();
    fs::write(
        specify_dir.join("project.yaml"),
        // Deliberate v1 shape — `schema:` instead of the post-RFC-13
        // `capability:` field.
        "name: demo\nschema: omnia\n",
    )
    .unwrap();

    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "status"]).assert().failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(
        value["error"], "schema-became-capability",
        "expected schema-became-capability diagnostic, got: {value}"
    );
    let msg = value["message"].as_str().expect("message string");
    assert!(msg.contains("schema-became-capability"), "msg: {msg}");
    assert!(msg.contains("capability"), "msg: {msg}");
    assert!(msg.contains("RFC-13"), "msg: {msg}");
    assert!(msg.contains(".specify/project.yaml"), "msg should name the file, got: {msg}");
}

#[test]
fn version_too_old_exits_three_with_json_envelope() {
    let tmp = tempdir().unwrap();
    // Fresh init to produce a real project.
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    // Pin a version far in the future.
    let config_path = tmp.path().join(".specify/project.yaml");
    let original = fs::read_to_string(&config_path).unwrap();
    let edited = original.replace(
        &format!("specify_version: {}", env!("CARGO_PKG_VERSION")),
        "specify_version: 99.0.0",
    );
    fs::write(&config_path, edited).unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "slice", "validate", "."])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("process exited with a code");
    assert_eq!(code, 3, "expected exit code 3 (version too old)");

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["error"], "specify-version-too-old");
    assert_eq!(value["exit-code"], 3);
}

// Change I's stub-subcommand assertion was retired in Change J; every
// subcommand now dispatches to real logic. End-to-end coverage of the
// wired subcommands lives in `tests/e2e.rs`.

// ---- specify init --hub (RFC-9 §1D platform-hub topology) ----

#[test]
fn init_hub_writes_canonical_on_disk_shape() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "init"])
        .args(["--name", "platform-hub", "--hub"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value["hub"], true, "JSON response must surface hub: true, got: {value}");
    assert_eq!(value["schema-name"], "hub");
    assert!(
        value["scaffolded-rule-keys"].as_array().expect("array").is_empty(),
        "hub init must not scaffold rule keys, got: {}",
        value["scaffolded-rule-keys"]
    );

    // RFC-13 chunk 2.9 — hub init scaffolds `project.yaml` (under
    // `.specify/`) plus `registry.yaml` at the repo root, and
    // nothing else. `registry.yaml` survives because bootstrapping a
    // hub *is* bootstrapping its registry; `change.md` and
    // `plan.yaml` stay operator-managed (minted via `specify change
    // create` / `specify change plan create` post-Phase-3.5).
    assert!(tmp.path().join(".specify/project.yaml").is_file());
    assert!(tmp.path().join("registry.yaml").is_file());
    for absent in ["initiative.md", "plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "hub init must not pre-touch `{absent}` at the repo root"
        );
    }
    // Phase-pipeline directories MUST NOT be present.
    assert!(!tmp.path().join(".specify/slices").exists());
    assert!(!tmp.path().join(".specify/specs").exists());
    assert!(!tmp.path().join(".specify/.cache").exists());

    // project.yaml shape — RFC-13 §Migration: `hub: true` only, no
    // `capability:` field, and the legacy `schema:` sentinel is gone.
    let project_yaml =
        fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read project.yaml");
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("schema:")),
        "post-RFC-13 hub project.yaml must omit the legacy `schema:` field:\n{project_yaml}"
    );
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("capability:")),
        "post-RFC-13 hub project.yaml must omit the `capability:` field:\n{project_yaml}"
    );
    assert!(
        project_yaml.contains("hub: true"),
        "project.yaml must carry `hub: true`:\n{project_yaml}"
    );

    // registry.yaml shape — version: 1, projects: [].
    let registry_yaml =
        fs::read_to_string(tmp.path().join("registry.yaml")).expect("read registry.yaml");
    assert!(
        registry_yaml.contains("version: 1"),
        "registry.yaml missing version:\n{registry_yaml}"
    );
    let registry: serde_json::Value =
        serde_yaml_to_json(&registry_yaml).expect("registry.yaml parses");
    assert_eq!(registry["version"], 1);
    assert!(
        registry["projects"].as_array().is_some_and(Vec::is_empty),
        "registry.yaml `projects` must be an empty list, got: {registry}"
    );

    // `change.md` is no longer scaffolded by hub init (RFC-13
    // chunk 2.9; pre-Phase-3.7 the brief filename was `initiative.md`).
    // The absence assertion above (in the on-disk shape block) is the
    // post-2.9 contract; a `change.md` body appears only after the
    // operator runs `specify change create <name>`.
}

#[test]
fn init_hub_refuses_when_specify_dir_already_exists() {
    let tmp = tempdir().unwrap();
    // Pre-create `.specify/` with arbitrary content.
    fs::create_dir_all(tmp.path().join(".specify")).unwrap();
    fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\ncapability: omnia\n")
        .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["init"])
        .args(["--name", "platform-hub", "--hub"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(
        stderr.contains("refusing to scaffold"),
        "stderr should explain the refusal, got: {stderr:?}"
    );

    let on_disk = fs::read_to_string(tmp.path().join(".specify/project.yaml")).unwrap();
    assert_eq!(on_disk, "name: existing\ncapability: omnia\n");
}

#[test]
fn init_hub_then_registry_validate_succeeds_on_empty_projects() {
    let tmp = tempdir().unwrap();
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .args(["--name", "platform-hub", "--hub"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["ok"], true);
}

#[test]
fn init_hub_then_registry_validate_rejects_dot_url_with_hub_diagnostic() {
    let tmp = tempdir().unwrap();
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .args(["--name", "platform-hub", "--hub"])
        .assert()
        .success();

    // Stomp the registry to pretend the operator hand-edited a `url: .`
    // entry. Hub-mode validation must surface the
    // `hub-cannot-be-project` diagnostic.
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: platform\n\
         \x20\x20\x20\x20url: .\n\
         \x20\x20\x20\x20schema: hub\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["ok"], false);
    let msg = value["error"].as_str().expect("error string");
    assert!(
        msg.contains("hub-cannot-be-project"),
        "error must carry the stable diagnostic code, got: {msg}"
    );
    assert!(msg.contains("registry.yaml"), "error must scope the file: {msg}");
}

/// Tiny YAML→JSON helper — we only need it for the hub on-disk shape
/// assertion, and pulling in a full yaml dependency for one test is
/// overkill. The registry file we write is shallow so a minimal hand
/// parser via `serde_json::from_str` after an indent-stripped
/// transform would be fragile; instead we re-use `serde_saphyr` (the
/// crate the rest of the CLI uses) by routing through a `Value`.
fn serde_yaml_to_json(yaml: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value =
        serde_saphyr::from_str(yaml).map_err(|err| format!("parse error: {err}"))?;
    Ok(value)
}

// ---- specify registry {add, remove} (RFC-9 §2A) ----

/// Scaffold a hub project; convenience for the registry-mutation tests
/// below. Hub mode gives us an empty `registry.yaml` to mutate without
/// having to seed an entry first.
fn init_hub(tmp: &tempfile::TempDir, name: &str) {
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .args(["--name", name, "--hub"])
        .assert()
        .success();
}

#[test]
fn registry_add_creates_entry_and_round_trips_through_show() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    // Hub registries reject `url: .` (hub-cannot-be-project) but accept
    // remote-url entries — the canonical multi-repo shape.
    let assert = specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "registry",
            "add",
            "alpha",
            "--url",
            "git@github.com:augentic/alpha.git",
            "--schema",
            "omnia@v1",
            "--description",
            "Alpha service",
        ])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["ok"], true);
    assert_eq!(value["added"]["name"], "alpha");
    assert_eq!(value["added"]["url"], "git@github.com:augentic/alpha.git");
    assert_eq!(value["added"]["schema"], "omnia@v1");
    assert_eq!(value["added"]["description"], "Alpha service");
    assert_eq!(value["registry"]["projects"].as_array().unwrap().len(), 1);

    // Round-trip via `registry show` — the entry must come back through
    // the canonical loader, not just the in-memory write path.
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "show"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let projects = value["registry"]["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "alpha");
}

#[test]
fn registry_add_rejects_dot_url_in_hub_mode() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "add", "self", "--url", ".", "--schema", "omnia@v1"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(
        msg.contains("hub-cannot-be-project"),
        "diagnostic must carry the stable code, got: {msg}"
    );
}

#[test]
fn registry_add_rejects_kebab_violations_at_clap_level() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    let assert = specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "registry",
            "add",
            "BadName",
            "--url",
            "git@github.com:org/bad.git",
            "--schema",
            "omnia@v1",
        ])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("kebab-case"), "diagnostic must mention kebab-case, got: {msg}");
    assert!(msg.contains("BadName"), "diagnostic must echo the bad name, got: {msg}");
}

#[test]
fn registry_remove_succeeds_and_round_trips() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    // Seed two entries — both descriptions present so the registry is
    // multi-repo–valid.
    for (name, url) in
        [("alpha", "git@github.com:org/alpha.git"), ("beta", "git@github.com:org/beta.git")]
    {
        specify()
            .current_dir(tmp.path())
            .args([
                "--format",
                "json",
                "registry",
                "add",
                name,
                "--url",
                url,
                "--schema",
                "omnia@v1",
                "--description",
                &format!("{name} service"),
            ])
            .assert()
            .success();
    }

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "remove", "beta"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["removed"], "beta");
    assert!(
        value["warnings"].as_array().expect("warnings array").is_empty(),
        "no plan.yaml present, warnings must be empty: {value}"
    );
    let projects = value["registry"]["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "alpha");
}

#[test]
fn registry_remove_warns_when_plan_references_project() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    for (name, url) in
        [("alpha", "git@github.com:org/alpha.git"), ("beta", "git@github.com:org/beta.git")]
    {
        specify()
            .current_dir(tmp.path())
            .args([
                "--format",
                "json",
                "registry",
                "add",
                name,
                "--url",
                url,
                "--schema",
                "omnia@v1",
                "--description",
                &format!("{name} service"),
            ])
            .assert()
            .success();
    }

    // Author a plan with one entry pointing at alpha.
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "create", "demo"])
        .assert()
        .success();
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "add", "alpha-feature", "--project", "alpha"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "remove", "alpha"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["removed"], "alpha");
    let warnings = value["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 1, "expected a single warning, got: {value}");
    let warning = warnings[0].as_str().expect("warning string");
    assert!(warning.contains("alpha-feature"), "warning must name the entry, got: {warning}");
    assert!(warning.contains("plan amend"), "warning must hint at remediation, got: {warning}");
}

#[test]
fn registry_remove_unknown_project_errors() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "remove", "ghost"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("not found"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
}

#[test]
fn registry_remove_refuses_when_registry_absent() {
    let tmp = tempdir().unwrap();
    // Plain init (no hub) — single-repo project has no registry.yaml
    // by default.
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();
    assert!(!tmp.path().join("registry.yaml").exists());

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "remove", "alpha"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("no registry declared"), "msg: {msg}");
}

// ---- specify workspace merge deprecation shim (RFC-14 C08) ----
//
// The shim must keep old invocations parseable for one release while
// refusing before any project, registry, PR lookup, or forge mutation.

#[test]
fn workspace_help_lists_merge_subcommand() {
    let assert = specify().args(["workspace", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["sync", "status", "push", "merge"] {
        assert!(
            stdout.contains(verb),
            "expected `workspace --help` to mention `{verb}`, got:\n{stdout}",
        );
    }
}

#[test]
fn workspace_merge_help_documents_dry_run_and_projects() {
    let assert = specify().args(["workspace", "merge", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(
        stdout.contains("Deprecated") || stdout.contains("removed"),
        "expected workspace merge help to explain removal, got:\n{stdout}",
    );
    assert!(
        stdout.contains("--dry-run"),
        "expected --dry-run flag in workspace merge --help, got:\n{stdout}",
    );
    // The positional `[PROJECTS]...` argument should be visible.
    assert!(
        stdout.to_lowercase().contains("project"),
        "expected projects positional in workspace merge --help, got:\n{stdout}",
    );
    assert!(
        stdout.contains("change finalize"),
        "expected workspace merge help to point at change finalize, got:\n{stdout}",
    );
}

#[test]
fn rfc14_c01_workspace_sync_unknown_selector_fails_before_side_effects() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();
    let gitignore_before = fs::read_to_string(tmp.path().join(".gitignore")).ok();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "sync", "ghost"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("unknown project selector"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace").exists(),
        "unknown selector must fail before workspace materialisation"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join(".gitignore")).ok(),
        gitignore_before,
        "unknown selector must fail before sync mutates .gitignore"
    );
}

#[test]
fn rfc14_c01_workspace_status_unknown_selector_fails_before_side_effects() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "status", "ghost"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("unknown project selector"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace").exists(),
        "status selector preflight must not materialise workspace paths"
    );
}

#[test]
fn rfc14_c01_workspace_sync_and_status_select_projects_in_registry_order() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    for name in ["billing", "orders", "inventory"] {
        fs::create_dir_all(tmp.path().join(name)).unwrap();
    }
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: billing\n\
         \x20\x20\x20\x20url: ./billing\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20description: billing service\n\
         \x20\x20- name: orders\n\
         \x20\x20\x20\x20url: ./orders\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20description: orders service\n\
         \x20\x20- name: inventory\n\
         \x20\x20\x20\x20url: ./inventory\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20description: inventory service\n",
    )
    .unwrap();

    specify()
        .current_dir(tmp.path())
        .args(["workspace", "sync", "orders", "billing"])
        .assert()
        .success();
    assert!(tmp.path().join(".specify/workspace/billing").exists());
    assert!(tmp.path().join(".specify/workspace/orders").exists());
    assert!(
        !tmp.path().join(".specify/workspace/inventory").exists(),
        "selected sync must not materialise unselected slots"
    );

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "status", "orders", "billing"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let names: Vec<&str> = value["slots"]
        .as_array()
        .expect("slots array")
        .iter()
        .map(|slot| slot["name"].as_str().expect("slot name"))
        .collect();
    assert_eq!(
        names,
        ["billing", "orders"],
        "selectors must preserve registry order, not argument order"
    );
}

#[test]
fn rfc14_c03_workspace_status_json_reports_enriched_slot_fields() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::create_dir_all(tmp.path().join("billing/.specify/slices/alpha")).unwrap();
    fs::write(
        tmp.path().join("billing/.specify/project.yaml"),
        "name: billing\ncapability: omnia@v1\n",
    )
    .unwrap();
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nchanges: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: billing\n\
         \x20\x20\x20\x20url: ./billing\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20description: billing service\n\
         \x20\x20- name: remote\n\
         \x20\x20\x20\x20url: git@github.com:org/remote.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20description: remote service\n",
    )
    .unwrap();

    specify().current_dir(tmp.path()).args(["workspace", "sync", "billing"]).assert().success();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "status", "remote", "billing"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let slots = value["slots"].as_array().expect("slots array");
    assert_eq!(slots.len(), 2);

    let billing = &slots[0];
    assert_eq!(billing["name"], "billing");
    assert_eq!(billing["kind"], "symlink");
    assert!(billing["slot-path"].as_str().unwrap().ends_with(".specify/workspace/billing"));
    assert_eq!(billing["configured-target-kind"], "local");
    assert!(billing["configured-target"].as_str().unwrap().ends_with("/billing"));
    assert!(billing["actual-symlink-target"].as_str().unwrap().ends_with("/billing"));
    assert_eq!(billing["project-config-present"], true);
    assert_eq!(billing["active-slices"], serde_json::json!(["alpha"]));
    assert!(billing["actual-origin"].is_null());
    assert!(billing["current-branch"].is_null());
    assert!(billing["branch-matches-change"].is_null());

    let remote = &slots[1];
    assert_eq!(remote["name"], "remote");
    assert_eq!(remote["kind"], "missing");
    assert_eq!(remote["configured-target-kind"], "remote");
    assert_eq!(remote["configured-target"], "git@github.com:org/remote.git");
    assert_eq!(remote["project-config-present"], false);
    assert_eq!(remote["active-slices"], serde_json::json!([]));
}

#[test]
fn rfc14_c03_workspace_status_text_flags_mismatch_dirty_and_project_config() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    let slot_path = tmp.path().join(".specify/workspace/remote");
    let remote_url = "git@github.com:org/remote.git";
    fs::create_dir_all(slot_path.join(".specify")).unwrap();
    fs::write(slot_path.join(".specify/project.yaml"), "name: remote\ncapability: omnia@v1\n")
        .unwrap();
    fs::write(slot_path.join("README.md"), "# remote\n").unwrap();
    run_git(&slot_path, &["init"]);
    run_git(&slot_path, &["remote", "add", "origin", remote_url]);
    run_git(&slot_path, &["add", "."]);
    run_git(&slot_path, &["commit", "-m", "initial"]);
    run_git(&slot_path, &["checkout", "-b", "feature/work"]);
    fs::write(slot_path.join("dirty.txt"), "dirty\n").unwrap();
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nchanges: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: remote\n\
         \x20\x20\x20\x20url: git@github.com:org/remote.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20description: remote service\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["workspace", "status", "remote"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");

    assert!(stdout.contains("remote: kind=git-clone"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("configured-remote=git@github.com:org/remote.git"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("origin=git@github.com:org/remote.git"), "stdout:\n{stdout}");
    assert!(stdout.contains("branch=feature/work"), "stdout:\n{stdout}");
    assert!(stdout.contains("change-branch=mismatch"), "stdout:\n{stdout}");
    assert!(stdout.contains("dirty=yes"), "stdout:\n{stdout}");
    assert!(stdout.contains("project.yaml=present"), "stdout:\n{stdout}");
}

#[test]
fn rfc14_c01_workspace_push_unknown_selector_fails_before_side_effects() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("plan.yaml"), "name: demo-change\nchanges: []\n").unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "push", "ghost", "--dry-run"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("unknown project selector"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
    assert!(
        !tmp.path().join(".specify/workspace").exists(),
        "unknown selector must fail before workspace paths are touched"
    );
}

#[test]
fn rfc14_c04_workspace_prepare_branch_hidden_helper_returns_structured_json() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    let alpha = tmp.path().join("alpha");
    fs::create_dir_all(&alpha).unwrap();
    run_git(&alpha, &["init", "-b", "main"]);
    fs::write(alpha.join("README.md"), "seed\n").unwrap();
    run_git(&alpha, &["add", "README.md"]);
    run_git(&alpha, &["commit", "--no-gpg-sign", "-m", "seed"]);
    let remote = tmp.path().join("alpha.git");
    run_git(tmp.path(), &["clone", "--bare", alpha.to_str().unwrap(), remote.to_str().unwrap()]);
    let remote_url = format!("file://{}", remote.display());
    run_git(&alpha, &["remote", "add", "origin", &remote_url]);

    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: ./alpha\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();

    let help = specify().args(["workspace", "--help"]).assert().success();
    let help_stdout = String::from_utf8(help.get_output().stdout.clone()).expect("help utf8");
    assert!(
        !help_stdout.contains("prepare-branch"),
        "executor helper must stay hidden from human workspace help, got:\n{help_stdout}"
    );

    let assert = specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "workspace",
            "prepare-branch",
            "alpha",
            "--change",
            "demo-change",
        ])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");

    assert_eq!(value["prepared"], true);
    assert_eq!(value["project"], "alpha");
    assert_eq!(value["branch"], "specify/demo-change");
    assert_eq!(value["local-branch"], "created");
    assert_eq!(value["remote-branch"], "absent");
    assert_eq!(value["dirty"]["tracked-blocked"], serde_json::json!([]));
    assert_eq!(value["diagnostics"], serde_json::json!([]));
    assert_eq!(run_git(&alpha, &["branch", "--show-current"]).trim(), "specify/demo-change");
}

#[test]
fn rfc14_c04_workspace_prepare_branch_surfaces_origin_head_diagnostic_key() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    let remote = tmp.path().join("headless.git");
    run_git(tmp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    let remote_url = format!("file://{}", remote.display());

    let alpha = tmp.path().join("alpha");
    fs::create_dir_all(&alpha).unwrap();
    run_git(&alpha, &["init", "-b", "main"]);
    run_git(&alpha, &["remote", "add", "origin", &remote_url]);
    fs::write(alpha.join("README.md"), "seed\n").unwrap();
    run_git(&alpha, &["add", "README.md"]);
    run_git(&alpha, &["commit", "--no-gpg-sign", "-m", "seed"]);

    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: ./alpha\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "workspace",
            "prepare-branch",
            "alpha",
            "--change",
            "demo-change",
        ])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");

    assert_eq!(value["error"], "branch-preparation-failed");
    assert_eq!(value["diagnostic"]["key"], "origin-head-unresolved");
    assert_eq!(value["diagnostic"]["project"], "alpha");
    assert_eq!(value["diagnostic"]["branch"], "specify/demo-change");
    assert_eq!(run_git(&alpha, &["branch", "--show-current"]).trim(), "main");
}

#[test]
fn workspace_merge_shim_exits_nonzero_with_migration_guidance() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "merge", "ghost", "--dry-run"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "workspace-merge-removed");
    assert_eq!(value["command"], "workspace merge");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["projects"], serde_json::json!(["ghost"]));
    assert_eq!(value["dry-run"], true);
    assert_eq!(value["inspected-pull-requests"], false);
    assert_eq!(value["merged-pull-requests"], false);
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("gh pr merge"), "msg: {msg}");
    assert!(msg.contains("change finalize"), "msg: {msg}");
}

#[test]
fn workspace_merge_shim_does_not_require_project_context_or_touch_workspace() {
    let tmp = tempdir().unwrap();
    let assert =
        specify().current_dir(tmp.path()).args(["workspace", "merge", "alpha"]).assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(stderr.contains("was removed"), "stderr: {stderr}");
    assert!(stderr.contains("change finalize"), "stderr: {stderr}");
    assert!(
        !tmp.path().join(".specify/workspace").exists(),
        "workspace merge shim must not materialise workspace paths"
    );
}

// ---- specify plan doctor (RFC-9 §4B) ----
//
// `plan doctor` is a strict superset of `plan validate`. The
// integration tests below pin the wire-shape skill authors will rely
// on: doctor MUST surface every diagnostic class on a synthetic
// fixture that exercises all four; validate on the same fixture MUST
// produce only the validate-level subset (proving doctor's additions
// are purely additive).

fn init_omnia_project(tmp: &tempfile::TempDir) {
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();
}

#[test]
fn plan_doctor_reports_all_four_diagnostic_classes() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    // Authoring a plan that intentionally exercises all four doctor
    // checks at once. We hand-write `plan.yaml` because the CLI's own
    // `plan create` path enforces validation at write time and would
    // refuse the cycle / unknown-source cases below.
    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
         sources:\n\
         \x20\x20monolith: /tmp/legacy\n\
         \x20\x20orphaned: /tmp/elsewhere\n\
         changes:\n\
         \x20\x20- name: cyclic-a\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [cyclic-b]\n\
         \x20\x20- name: cyclic-b\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [cyclic-a]\n\
         \x20\x20- name: failed-root\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: failed\n\
         \x20\x20\x20\x20status-reason: regression in upstream service\n\
         \x20\x20- name: unreachable-leaf\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [failed-root]\n\
         \x20\x20- name: orphaned-source-user\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20sources: [monolith]\n",
    )
    .unwrap();

    // Hand-write a registry at the repo root, so we can exercise
    // stale-clone with a deterministic fixture: a clone slot whose
    // origin remote disagrees with the registry.
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();
    let slot = tmp.path().join(".specify/workspace/alpha");
    fs::create_dir_all(&slot).unwrap();
    let init = ProcessCommand::new("git").arg("-C").arg(&slot).arg("init").output().unwrap();
    assert!(init.status.success(), "git init failed: {}", String::from_utf8_lossy(&init.stderr));
    let remote = ProcessCommand::new("git")
        .arg("-C")
        .arg(&slot)
        .args(["remote", "add", "origin", "git@github.com:old/alpha.git"])
        .output()
        .unwrap();
    assert!(
        remote.status.success(),
        "git remote add failed: {}",
        String::from_utf8_lossy(&remote.stderr)
    );

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "doctor"])
        .assert();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["ok"], false, "errors must mark ok=false: {value}");

    let diagnostics = value["diagnostics"].as_array().expect("diagnostics array");
    let codes: Vec<&str> = diagnostics.iter().filter_map(|d| d["code"].as_str()).collect();

    for expected in
        ["cycle-in-depends-on", "orphan-source-key", "stale-workspace-clone", "unreachable-entry"]
    {
        assert!(
            codes.contains(&expected),
            "doctor must emit `{expected}` for the synthetic fixture; saw: {codes:?}"
        );
    }

    // Exit code must be ValidationFailed (2) because cycle and
    // unreachable-entry are error-severity.
    let code = output.status.code().expect("exit code");
    assert_eq!(code, 2, "error-severity diagnostics must yield exit 2, got {code}");
}

#[test]
fn plan_doctor_diagnostic_payloads_round_trip_typed() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    // Minimal plan that exercises just the cycle and orphan-source
    // checks — enough to confirm the typed payload deserialises
    // cleanly.
    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
         sources:\n\
         \x20\x20orphan-key: /tmp/somewhere\n\
         changes:\n\
         \x20\x20- name: cyc-a\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [cyc-b]\n\
         \x20\x20- name: cyc-b\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [cyc-a]\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "doctor"])
        .assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    let diagnostics = value["diagnostics"].as_array().expect("diagnostics array");

    let cycle = diagnostics
        .iter()
        .find(|d| d["code"] == "cycle-in-depends-on")
        .expect("expected cycle-in-depends-on diagnostic");
    let cycle_path = cycle["data"]["cycle"].as_array().expect("cycle path is array");
    let names: Vec<String> =
        cycle_path.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    assert_eq!(
        names,
        vec!["cyc-a".to_string(), "cyc-b".to_string(), "cyc-a".to_string()],
        "cycle path must close on the first node"
    );
    assert_eq!(cycle["data"]["kind"], "cycle");

    let orphan = diagnostics
        .iter()
        .find(|d| d["code"] == "orphan-source-key")
        .expect("expected orphan-source-key diagnostic");
    assert_eq!(orphan["data"]["kind"], "orphan-source");
    assert_eq!(orphan["data"]["key"], "orphan-key");
    assert_eq!(orphan["severity"], "warning");
}

#[test]
fn plan_validate_unchanged_by_doctor_fixture() {
    // Same fixture as `plan_doctor_reports_all_four_diagnostic_classes`
    // but routed through `plan validate` — only the validate-level
    // subset must surface; the four doctor codes must be absent.
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
         sources:\n\
         \x20\x20monolith: /tmp/legacy\n\
         \x20\x20orphaned: /tmp/elsewhere\n\
         changes:\n\
         \x20\x20- name: cyclic-a\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [cyclic-b]\n\
         \x20\x20- name: cyclic-b\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [cyclic-a]\n\
         \x20\x20- name: failed-root\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: failed\n\
         \x20\x20\x20\x20status-reason: regression in upstream service\n\
         \x20\x20- name: unreachable-leaf\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20depends-on: [failed-root]\n\
         \x20\x20- name: orphaned-source-user\n\
         \x20\x20\x20\x20schema: omnia@v1\n\
         \x20\x20\x20\x20status: pending\n\
         \x20\x20\x20\x20sources: [monolith]\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "validate"])
        .assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    let codes: Vec<&str> = value["results"]
        .as_array()
        .expect("results array")
        .iter()
        .filter_map(|r| r["code"].as_str())
        .collect();

    // validate's existing cycle code must still fire.
    assert!(
        codes.contains(&"dependency-cycle"),
        "validate must continue to emit dependency-cycle, got: {codes:?}"
    );
    // None of doctor's four new codes should leak into validate.
    for doctor_code in
        ["cycle-in-depends-on", "orphan-source-key", "stale-workspace-clone", "unreachable-entry"]
    {
        assert!(
            !codes.contains(&doctor_code),
            "validate must NOT emit doctor-only code `{doctor_code}`; got: {codes:?}"
        );
    }
}

#[test]
fn plan_doctor_healthy_plan_exits_zero() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "create", "demo"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "plan", "doctor"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["ok"], true, "empty plan must be ok");
    assert_eq!(
        value["diagnostics"].as_array().unwrap().len(),
        0,
        "empty plan must emit zero diagnostics"
    );
}

#[test]
fn plan_doctor_help_documents_superset_relationship() {
    let assert = specify().args(["change", "plan", "doctor", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for code in
        ["cycle-in-depends-on", "orphan-source-key", "stale-workspace-clone", "unreachable-entry"]
    {
        assert!(
            stdout.contains(code),
            "plan doctor --help must document the `{code}` diagnostic; got:\n{stdout}"
        );
    }
}

// ---- specify change finalize (RFC-9 §4C) ----
//
// Wire-level integration tests for the precondition diagnostics. The
// happy-path classifier flow is covered by the in-process `MockProbe`
// against the orchestrator (see `cargo test -p specify-change`).
// The CLI tests below pin: (a) the failure-mode wire shape skill
// authors will rely on, and (b) the on-disk archive landing when no
// projects need probing.

#[test]
fn change_help_lists_finalize_subcommand() {
    let assert = specify().args(["change", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["create", "show", "finalize"] {
        assert!(
            stdout.contains(verb),
            "expected `change --help` to mention `{verb}`, got:\n{stdout}",
        );
    }
}

#[test]
fn change_finalize_help_documents_clean_and_dry_run() {
    let assert = specify().args(["change", "finalize", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for flag in ["--clean", "--dry-run"] {
        assert!(stdout.contains(flag), "expected --help to document `{flag}`, got:\n{stdout}");
    }
    assert!(
        stdout.contains("operator-merged") || stdout.contains("gh pr merge"),
        "finalize help must explain PRs are operator-merged before finalize, got:\n{stdout}",
    );
}

#[test]
fn change_finalize_refuses_when_plan_absent() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    assert!(!tmp.path().join("plan.yaml").exists(), "test precondition: plan.yaml must be absent");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "plan-not-found");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("plan.yaml"), "msg should reference plan.yaml: {msg}");
    // Diagnostic should hint at the recovery sequence — post-Phase-3.5
    // the umbrella surface is `specify change plan create` (and
    // `specify change create` for the brief).
    assert!(
        msg.contains("plan create") || msg.contains("change create"),
        "msg should hint at the change/plan create verbs, got: {msg}",
    );
}

#[test]
fn change_finalize_refuses_on_non_terminal_entries() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Seed a plan with one done and one pending entry — pending is
    // not terminal for finalize.
    fs::write(
        tmp.path().join("plan.yaml"),
        "name: foo\n\
         changes:\n\
         \x20\x20- name: a\n\
         \x20\x20\x20\x20schema: contracts@v1\n\
         \x20\x20\x20\x20status: done\n\
         \x20\x20- name: b\n\
         \x20\x20\x20\x20schema: contracts@v1\n\
         \x20\x20\x20\x20status: pending\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "non-terminal-entries-present");
    assert_eq!(value["initiative"], "foo");
    let entries = value["entries"].as_array().expect("entries array");
    let names: Vec<&str> = entries.iter().filter_map(|e| e.as_str()).collect();
    assert_eq!(names, ["b"], "entries must list the offending non-terminal name");

    // Atomicity: plan.yaml must remain on disk on refusal.
    assert!(tmp.path().join("plan.yaml").exists(), "plan.yaml must be untouched");
}

#[test]
fn change_finalize_dry_run_archives_nothing_with_empty_registry() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Seed an all-terminal plan and rely on the hub-init's empty
    // registry — no per-project probes will run.
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nchanges: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize", "--dry-run"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["initiative"], "foo");
    assert_eq!(value["finalized"], true);
    assert_eq!(value["dry-run"], true, "dry-run flag must echo into JSON");
    assert!(value.get("archived").is_none(), "dry-run must not stamp archived path");
    let projects = value["projects"].as_array().expect("projects array");
    assert!(projects.is_empty(), "empty registry → empty projects, got: {projects:?}");

    // On-disk: plan.yaml must remain.
    assert!(tmp.path().join("plan.yaml").exists(), "dry-run must not move plan.yaml");
}

#[test]
fn change_finalize_archives_when_all_terminal_and_no_registry() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nchanges: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["initiative"], "foo");
    assert_eq!(value["finalized"], true);
    let archived = value["archived"].as_str().expect("archived path");
    assert!(archived.contains("foo-"), "archived path must contain plan name: {archived}");
    let summary = value["summary"].as_object().expect("summary object");
    for key in
        ["merged", "unmerged", "closed", "no-branch", "branch-pattern-mismatch", "dirty", "failed"]
    {
        assert!(summary.contains_key(key), "summary missing `{key}`: {summary:?}");
    }

    // Plan.yaml must have moved into the archive.
    assert!(!tmp.path().join("plan.yaml").exists(), "plan.yaml must be archived");
    let archive_dir = tmp.path().join(".specify/archive/plans");
    let entries: Vec<String> = fs::read_dir(&archive_dir)
        .expect("read archive dir")
        .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
        .collect();
    assert!(
        entries.iter().any(|n| n.starts_with("foo-")),
        "archive dir should contain a foo-<YYYYMMDD>.yaml: {entries:?}",
    );
}

#[test]
fn change_finalize_idempotent_after_archive() {
    // Idempotency proof: the second `finalize` invocation after the
    // archive landed produces a clear `plan-not-found` refusal — the
    // canonical "change is already finalized" signal.
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nchanges: []\n").unwrap();

    // First run: archives the plan.
    specify().current_dir(tmp.path()).args(["change", "finalize"]).assert().success();
    assert!(!tmp.path().join("plan.yaml").exists());

    // Second run: plan is gone, refused with plan-not-found.
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "plan-not-found");
}

// ---- specify migrate v2-layout (Option-3 hard cutover) ----

/// Seed a v1-layout project: a real `.specify/` (so `ProjectConfig::load`
/// succeeds) plus the four legacy artifacts the detector watches for.
/// Used by the `legacy-layout` cutover and `migrate v2-layout` tests.
fn seed_v1_layout(tmp: &tempfile::TempDir) {
    init_hub(tmp, "platform-hub");
    let specify = tmp.path().join(".specify");
    // Hub init writes `registry.yaml` at the repo root (per RFC-13
    // chunk 2.9, that's the one platform-component artefact init
    // touches); move it back to `.specify/` to simulate a v1-layout
    // project that needs migrating.
    fs::rename(tmp.path().join("registry.yaml"), specify.join("registry.yaml"))
        .expect("move registry.yaml back to .specify/");
    // `initiative.md` and `plan.yaml` are no longer scaffolded by
    // init (operator-managed via `specify initiative create` /
    // `specify plan create`). Hand-seed them at the v1 location so
    // the detector and migrator have all four legacy artefacts to
    // act on.
    fs::write(specify.join("initiative.md"), "---\nname: demo\ninputs: []\n---\n\n# demo\n")
        .expect("seed initiative.md");
    fs::write(specify.join("plan.yaml"), "name: demo\nchanges: []\n").expect("seed plan.yaml");
    let contracts = specify.join("contracts").join("schemas");
    fs::create_dir_all(&contracts).expect("mkdir .specify/contracts/schemas");
    fs::write(contracts.join("payload.yaml"), "type: object\n").expect("seed contract");
}

#[test]
fn project_aware_command_refuses_on_v1_layout_with_legacy_layout_error() {
    let tmp = tempdir().unwrap();
    seed_v1_layout(&tmp);

    // `specify status` is the canonical project-aware entry point; the
    // detector wired into `run_with_project` must surface
    // `legacy-layout` before the dashboard even runs.
    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "status"]).assert().failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "legacy-layout");
    let msg = value["message"].as_str().expect("message");
    assert!(
        msg.contains(".specify/registry.yaml"),
        "legacy-layout message must enumerate the offenders, got: {msg}"
    );
    assert!(
        msg.contains("specify migrate v2-layout"),
        "legacy-layout message must point at the migrate verb, got: {msg}"
    );
}

#[test]
fn migrate_v2_layout_moves_every_artifact_and_succeeds() {
    let tmp = tempdir().unwrap();
    seed_v1_layout(&tmp);

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "v2-layout"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["any-collisions"], false);
    assert_eq!(value["any-legacy-present"], true);

    // v2 destinations now exist at the repo root.
    assert!(tmp.path().join("registry.yaml").is_file());
    assert!(tmp.path().join("plan.yaml").is_file());
    assert!(tmp.path().join("initiative.md").is_file());
    assert!(tmp.path().join("contracts").is_dir());

    // v1 sources are gone.
    assert!(!tmp.path().join(".specify/registry.yaml").exists());
    assert!(!tmp.path().join(".specify/plan.yaml").exists());
    assert!(!tmp.path().join(".specify/initiative.md").exists());
    assert!(!tmp.path().join(".specify/contracts").exists());

    // Re-running on the migrated repo is a no-op.
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "v2-layout"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["any-legacy-present"], false);

    // After the migration, project-aware verbs work again.
    specify().current_dir(tmp.path()).args(["registry", "show"]).assert().success();
}

#[test]
fn migrate_v2_layout_dry_run_does_not_modify_disk() {
    let tmp = tempdir().unwrap();
    seed_v1_layout(&tmp);

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "v2-layout", "--dry-run"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["dry-run"], true);

    // Sources still in .specify/, no destinations at root.
    assert!(tmp.path().join(".specify/registry.yaml").is_file());
    assert!(!tmp.path().join("registry.yaml").exists());
}

#[test]
fn migrate_v2_layout_refuses_destination_collision() {
    let tmp = tempdir().unwrap();
    seed_v1_layout(&tmp);
    // Plant a colliding file at the v2 destination.
    fs::write(tmp.path().join("registry.yaml"), "pre-existing\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "v2-layout"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["any-collisions"], true);

    // Pre-existing destination must not be clobbered.
    let pre = fs::read_to_string(tmp.path().join("registry.yaml")).expect("read pre-existing");
    assert_eq!(pre, "pre-existing\n");
    // v1 source must still be on disk so the operator can resolve.
    assert!(tmp.path().join(".specify/registry.yaml").is_file());
}

// ---- specify migrate slice-layout (RFC-13 chunk 3.6) ----

/// Stand up a v1 (pre-Phase-3) project on disk: a real `.specify/`
/// (so the migrator's "is this a Specify project?" preflight passes)
/// plus `.specify/changes/<name>/.metadata.yaml` for each requested
/// slice. `init_omnia_project` already creates `.specify/slices/`
/// (post-Phase-3 layout); the helper removes it so the migrator sees
/// only the v1 source directory and can apply the rename cleanly.
fn seed_pre_phase3_layout(tmp: &tempfile::TempDir, slices: &[(&str, &str)]) {
    init_omnia_project(tmp);
    let post_phase3 = tmp.path().join(".specify/slices");
    if post_phase3.is_dir() {
        fs::remove_dir_all(&post_phase3).expect("remove post-Phase-3 slices dir");
    }
    let changes_dir = tmp.path().join(".specify/changes");
    fs::create_dir_all(&changes_dir).expect("mkdir .specify/changes");
    for (name, status) in slices {
        let entry = changes_dir.join(name);
        fs::create_dir_all(&entry).expect("mkdir slice dir");
        // Minimal metadata shape that the slice crate's `serde_saphyr`
        // reader accepts: schema + lifecycle status are required, the
        // rest of the fields default through `#[serde(default)]`.
        fs::write(entry.join(".metadata.yaml"), format!("schema: omnia\nstatus: {status}\n"))
            .expect("seed .metadata.yaml");
    }
}

#[test]
fn migrate_slice_layout_renames_changes_dir_and_preserves_metadata() {
    let tmp = tempdir().unwrap();
    seed_pre_phase3_layout(&tmp, &[("alpha", "merged"), ("beta", "dropped")]);
    // Plant a non-metadata payload to confirm the rename moves the
    // entire subtree, not just the metadata files.
    fs::write(tmp.path().join(".specify/changes/alpha/notes.md"), "# alpha\n")
        .expect("seed payload");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["status"], "migrated");
    assert_eq!(value["slices-moved"], 2);

    // .specify/changes/ is gone; .specify/slices/ carries the
    // migrated subtree verbatim (metadata + ad-hoc payload).
    assert!(!tmp.path().join(".specify/changes").exists(), "v1 source must be removed");
    assert!(tmp.path().join(".specify/slices/alpha/.metadata.yaml").is_file());
    assert!(tmp.path().join(".specify/slices/beta/.metadata.yaml").is_file());
    let payload = fs::read_to_string(tmp.path().join(".specify/slices/alpha/notes.md"))
        .expect("payload survived rename");
    assert_eq!(payload, "# alpha\n");
}

#[test]
fn migrate_slice_layout_rerun_is_a_noop() {
    let tmp = tempdir().unwrap();
    seed_pre_phase3_layout(&tmp, &[("alpha", "merged")]);

    // First run actually migrates.
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .success();

    // Second run on the now-post-Phase-3 layout exits 0 with the
    // `already-migrated` discriminant — idempotency invariant.
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["status"], "already-migrated");
    assert_eq!(value["slices-moved"], 0);
    // The post-Phase-3 layout must still be intact after the no-op.
    assert!(tmp.path().join(".specify/slices/alpha").is_dir());
    assert!(!tmp.path().join(".specify/changes").exists());
}

#[test]
fn migrate_slice_layout_blocks_when_a_slice_is_in_progress() {
    let tmp = tempdir().unwrap();
    // `defining` is the canonical first non-terminal lifecycle state;
    // `merged` confirms the preflight ignores already-terminal slices.
    seed_pre_phase3_layout(&tmp, &[("alpha", "merged"), ("zeta", "defining")]);

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .failure();

    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "slice-migration-blocked-by-in-progress");
    let message = value["message"].as_str().expect("message string");
    assert!(
        message.contains("slice-migration-blocked-by-in-progress:"),
        "diagnostic must lead with the kebab-case prefix, got: {message}"
    );
    assert!(
        message.contains("zeta (defining)"),
        "diagnostic must surface the in-progress (name, status) pair, got: {message}"
    );
    assert!(
        !message.contains("alpha"),
        "terminal slices must not appear in the offender list, got: {message}"
    );

    // The on-disk state must be untouched: the v1 source still in
    // place, the post-Phase-3 destination absent. The operator's
    // recovery path is `specify slice drop zeta` followed by re-run.
    assert!(tmp.path().join(".specify/changes/zeta/.metadata.yaml").is_file());
    assert!(tmp.path().join(".specify/changes/alpha/.metadata.yaml").is_file());
    assert!(!tmp.path().join(".specify/slices").exists());
}

// ---- specify migrate change-noun (RFC-13 chunk 3.7) ----

/// Minimal `initiative.md` body used by the change-noun integration
/// tests. The migration is a wholesale `fs::rename`, so the body
/// bytes survive verbatim across the rename.
const CHANGE_NOUN_BODY: &str = "---\nname: demo\ninputs: []\n---\n\n# demo\n";

#[test]
fn migrate_change_noun_renames_initiative_to_change_md() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);
    fs::write(tmp.path().join("initiative.md"), CHANGE_NOUN_BODY).expect("seed initiative.md");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["status"], "migrated");
    assert_eq!(value["renamed-from"], "initiative.md");
    assert_eq!(value["renamed-to"], "change.md");

    // Source removed, destination present with byte-identical content.
    assert!(!tmp.path().join("initiative.md").exists(), "v1 source must be removed");
    assert!(tmp.path().join("change.md").is_file(), "post-rename file must exist");
    let migrated = fs::read_to_string(tmp.path().join("change.md")).expect("read change.md");
    assert_eq!(migrated, CHANGE_NOUN_BODY, "rename must preserve the body verbatim");
}

#[test]
fn migrate_change_noun_rerun_is_a_noop() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);
    fs::write(tmp.path().join("initiative.md"), CHANGE_NOUN_BODY).expect("seed initiative.md");

    // First run actually migrates.
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .success();

    // Second run on the now-post-Phase-3.7 layout exits 0 with the
    // `already-migrated` discriminant — idempotency invariant.
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["status"], "already-migrated");
    // `renamed-from` / `renamed-to` are omitted on no-op statuses so
    // JSON consumers can branch on presence.
    assert!(value.get("renamed-from").is_none(), "got: {value}");
    assert!(value.get("renamed-to").is_none(), "got: {value}");
    assert!(tmp.path().join("change.md").is_file());
    assert!(!tmp.path().join("initiative.md").exists());
}

#[test]
fn migrate_change_noun_refuses_when_both_files_present() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);
    fs::write(tmp.path().join("initiative.md"), CHANGE_NOUN_BODY).expect("seed initiative.md");
    fs::write(
        tmp.path().join("change.md"),
        "---\nname: collision\ninputs: []\n---\n\n# collision\n",
    )
    .expect("seed change.md");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "change-noun-migration-target-exists");
    let message = value["message"].as_str().expect("message string");
    assert!(
        message.contains("change-noun-migration-target-exists:"),
        "diagnostic must lead with the kebab-case prefix, got: {message}"
    );
    assert!(
        message.contains("specify migrate change-noun"),
        "diagnostic must reference the recovery verb, got: {message}"
    );

    // The on-disk state must be untouched.
    assert_eq!(
        fs::read_to_string(tmp.path().join("initiative.md")).expect("read"),
        CHANGE_NOUN_BODY
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("change.md")).expect("read"),
        "---\nname: collision\ninputs: []\n---\n\n# collision\n"
    );
}

/// `specify change show` against a project that still carries the
/// pre-Phase-3.7 `initiative.md` (and no `change.md`) must refuse
/// loud with the `change-brief-became-change-md` diagnostic and
/// point the operator at `specify migrate change-noun`.
#[test]
fn change_show_refuses_when_only_legacy_brief_present() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);
    fs::write(tmp.path().join("initiative.md"), CHANGE_NOUN_BODY).expect("seed initiative.md");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "show"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "change-brief-became-change-md");
    let message = value["message"].as_str().expect("message string");
    assert!(
        message.contains("change-brief-became-change-md:"),
        "diagnostic must lead with the kebab-case prefix, got: {message}"
    );
    assert!(
        message.contains("initiative.md") && message.contains("change.md"),
        "diagnostic must surface both filenames, got: {message}"
    );
    assert!(
        message.contains("specify migrate change-noun"),
        "diagnostic must point at the migration verb, got: {message}"
    );
    assert!(
        message.contains("rfcs/rfc-13-extensibility.md#migration"),
        "diagnostic must link RFC-13 §Migration, got: {message}"
    );
    // The on-disk state must be untouched.
    assert!(tmp.path().join("initiative.md").is_file());
    assert!(!tmp.path().join("change.md").exists());
}

/// Help text must list the new migration subcommand so operators can
/// discover it through `specify migrate --help`.
#[test]
fn migrate_change_noun_appears_in_help() {
    let assert = specify().args(["migrate", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("change-noun"), "migrate --help must list change-noun, got:\n{stdout}");
}

// ---- RFC-13 Phase 3.8 — migration round-trip acceptance gate ----
//
// The two Phase-3 migrations (`slice-layout` from chunk 3.6 and
// `change-noun` from chunk 3.7) ship with their own unit and
// integration tests. The acceptance gate for chunk 3.8, however, is
// the operator's lived experience: starting from a real v1 project,
// running the migrations should leave a tree that the post-RFC-13
// surface verbs (`specify slice list`, `specify change show`)
// drive without further intervention.
//
// Each test below stands up a fresh tempdir, seeds the v1-shaped
// artefacts, runs the relevant migration(s) end-to-end, and then
// drives the post-Phase-3 verb against the migrated tree to prove
// it is a working post-RFC-13 project.

/// Round-trip: a v1 project with `.specify/changes/<name>/` and a
/// `change.md` brief (i.e. only the slice-layout rename is needed)
/// migrates to `.specify/slices/<name>/`, and `specify slice list`
/// against the migrated tree surfaces every legacy slice exactly as
/// `slice create` would have minted.
#[test]
fn migration_roundtrip_slice_layout_makes_slice_discoverable() {
    let tmp = tempdir().unwrap();
    seed_pre_phase3_layout(&tmp, &[("alpha", "merged"), ("beta", "merged")]);

    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .success();

    assert!(tmp.path().join(".specify/slices/alpha/.metadata.yaml").is_file());
    assert!(tmp.path().join(".specify/slices/beta/.metadata.yaml").is_file());
    assert!(!tmp.path().join(".specify/changes").exists(), "v1 layout must be gone");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "slice", "list"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let names: Vec<&str> = value["slices"]
        .as_array()
        .expect("slices array")
        .iter()
        .map(|s| s["name"].as_str().expect("slice name"))
        .collect();
    assert_eq!(
        names,
        ["alpha", "beta"],
        "post-migration `specify slice list` must surface every legacy slice, got: {names:?}"
    );
}

/// Round-trip: a v1 project that carries the pre-Phase-3.7
/// `initiative.md` brief (and no `change.md`) migrates to
/// `change.md`, and `specify change show` reads it back through the
/// canonical loader.
#[test]
fn migration_roundtrip_change_noun_makes_brief_readable_through_show() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);
    fs::write(tmp.path().join("initiative.md"), CHANGE_NOUN_BODY).expect("seed v1 initiative.md");

    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .success();
    assert!(tmp.path().join("change.md").is_file());
    assert!(!tmp.path().join("initiative.md").exists());

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "show"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let brief = value["brief"].as_object().expect("brief object");
    assert_eq!(brief["frontmatter"]["name"], "demo");
    let path = value["path"].as_str().expect("path string");
    assert!(
        path.ends_with("/change.md"),
        "post-migration `specify change show` must point at change.md, got: {path}"
    );
}

/// Round-trip: a project that carries both v1 shapes — the
/// `.specify/changes/` slice layout *and* the `initiative.md` brief
/// — runs both migrations in the order `slice-layout` then
/// `change-noun`, and ends up indistinguishable (modulo operator
/// data) from a fresh post-RFC-13 project. Both post-Phase-3 verbs
/// (`specify slice list`, `specify change show`) drive against the
/// migrated tree without re-running any migration.
#[test]
fn migration_roundtrip_combined_slice_layout_then_change_noun() {
    let tmp = tempdir().unwrap();
    seed_pre_phase3_layout(&tmp, &[("alpha", "merged")]);
    fs::write(tmp.path().join("initiative.md"), CHANGE_NOUN_BODY).expect("seed v1 initiative.md");

    // Run the migrations in the documented order: slice-layout
    // first, then change-noun. The order matters because
    // `change-noun` only touches a repo-root file and is independent
    // of the slice tree, but the documented operator path is
    // slice-layout → change-noun.
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .success();
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .success();

    // Post-Phase-3 layout is in place: slices live under
    // `.specify/slices/`, the brief is `change.md`, and neither v1
    // artefact remains.
    assert!(tmp.path().join(".specify/slices/alpha/.metadata.yaml").is_file());
    assert!(tmp.path().join("change.md").is_file());
    assert!(!tmp.path().join(".specify/changes").exists());
    assert!(!tmp.path().join("initiative.md").exists());

    // Both post-Phase-3 surface verbs drive the migrated tree
    // without prompting another migration.
    let list = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "slice", "list"])
        .assert()
        .success();
    let list_value: serde_json::Value =
        serde_json::from_slice(&list.get_output().stdout).expect("json");
    let names: Vec<&str> = list_value["slices"]
        .as_array()
        .expect("slices array")
        .iter()
        .map(|s| s["name"].as_str().expect("slice name"))
        .collect();
    assert_eq!(names, ["alpha"]);

    let show = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "show"])
        .assert()
        .success();
    let show_value: serde_json::Value =
        serde_json::from_slice(&show.get_output().stdout).expect("json");
    assert_eq!(show_value["brief"]["frontmatter"]["name"], "demo");

    // Re-running either migration on the post-Phase-3 tree is a
    // documented no-op (idempotency) — the round-trip is stable.
    let rerun_slice = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "slice-layout"])
        .assert()
        .success();
    let rerun_slice_value: serde_json::Value =
        serde_json::from_slice(&rerun_slice.get_output().stdout).expect("json");
    assert_eq!(rerun_slice_value["status"], "already-migrated");

    let rerun_noun = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "migrate", "change-noun"])
        .assert()
        .success();
    let rerun_noun_value: serde_json::Value =
        serde_json::from_slice(&rerun_noun.get_output().stdout).expect("json");
    assert_eq!(rerun_noun_value["status"], "already-migrated");
}
