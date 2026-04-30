//! Integration tests for the `specify` CLI binary.
//!
//! Each test spawns the built binary via `assert_cmd::Command::cargo_bin`
//! from a fresh `tempfile::TempDir`, so stdout/stderr and filesystem side
//! effects are observed exactly as a user would experience them.

use std::fs;
use std::path::PathBuf;

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
        .args(["init", "--schema-uri"])
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
        .args(["--format", "json", "init", "--schema-uri"])
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
        .args([
            "init",
            "--schema-uri",
            "https://github.com/augentic/specify/schemas/omnia",
            "--name",
            "demo",
        ])
        .assert()
        .success();
}

#[test]
fn version_too_old_exits_three_with_json_envelope() {
    let tmp = tempdir().unwrap();
    // Fresh init to produce a real project.
    specify()
        .current_dir(tmp.path())
        .args(["init", "--schema-uri"])
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
        .args(["--format", "json", "change", "validate", "."])
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

    // Files we expect.
    assert!(tmp.path().join(".specify/project.yaml").is_file());
    assert!(tmp.path().join(".specify/registry.yaml").is_file());
    assert!(tmp.path().join(".specify/initiative.md").is_file());
    // Phase-pipeline directories MUST NOT be present.
    assert!(!tmp.path().join(".specify/changes").exists());
    assert!(!tmp.path().join(".specify/specs").exists());
    assert!(!tmp.path().join(".specify/.cache").exists());

    // project.yaml shape.
    let project_yaml =
        fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read project.yaml");
    assert!(
        project_yaml.contains("schema: hub"),
        "project.yaml must use the `schema: hub` sentinel:\n{project_yaml}"
    );
    assert!(
        project_yaml.contains("hub: true"),
        "project.yaml must carry `hub: true`:\n{project_yaml}"
    );

    // registry.yaml shape — version: 1, projects: [].
    let registry_yaml =
        fs::read_to_string(tmp.path().join(".specify/registry.yaml")).expect("read registry.yaml");
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

    // initiative.md shape — name baked into frontmatter.
    let brief = fs::read_to_string(tmp.path().join(".specify/initiative.md")).expect("read brief");
    assert!(
        brief.contains("name: platform-hub"),
        "initiative.md missing kebab-cased name:\n{brief}"
    );
}

#[test]
fn init_hub_refuses_when_specify_dir_already_exists() {
    let tmp = tempdir().unwrap();
    // Pre-create `.specify/` with arbitrary content.
    fs::create_dir_all(tmp.path().join(".specify")).unwrap();
    fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\nschema: omnia\n").unwrap();

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
    assert_eq!(on_disk, "name: existing\nschema: omnia\n");
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
        tmp.path().join(".specify/registry.yaml"),
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
        .args(["--format", "json", "plan", "create", "demo"])
        .assert()
        .success();
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "plan", "add", "alpha-feature", "--project", "alpha"])
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
        .args(["init", "--schema-uri"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();
    assert!(!tmp.path().join(".specify/registry.yaml").exists());

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

// ---- specify workspace merge (RFC-9 §4A) ----
//
// CLI-level integration tests for the precondition diagnostics. The
// happy-path classifier flow is covered by `cargo test --lib
// workspace_merge` against an in-process `MockGh` (the function-pointer
// alternative the brief explicitly allows); the assertions below pin
// the failure-mode wire shape skill authors will rely on.

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
        stdout.contains("--dry-run"),
        "expected --dry-run flag in workspace merge --help, got:\n{stdout}",
    );
    // The positional `[PROJECTS]...` argument should be visible.
    assert!(
        stdout.to_lowercase().contains("project"),
        "expected projects positional in workspace merge --help, got:\n{stdout}",
    );
}

#[test]
fn workspace_merge_refuses_when_plan_absent() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");

    // Seed a registry entry so the *plan* check is the first guard hit.
    specify()
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "registry",
            "add",
            "alpha",
            "--url",
            "git@github.com:org/alpha.git",
            "--schema",
            "omnia@v1",
            "--description",
            "alpha service",
        ])
        .assert()
        .success();

    assert!(
        !tmp.path().join(".specify/plan.yaml").exists(),
        "test precondition: plan.yaml must be absent",
    );

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "merge"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("plan.yaml"), "diagnostic must reference plan.yaml, got: {msg}");
    // Surface the operator remediation hint per the brief
    // ("diagnostic should point at `specify ... initiative {init,create}`
    // and `specify plan {init,create}`").
    assert!(
        msg.contains("plan init") || msg.contains("plan create"),
        "diagnostic must hint at the plan-init verb, got: {msg}",
    );
}

#[test]
fn workspace_merge_refuses_when_registry_absent() {
    let tmp = tempdir().unwrap();
    // Plain init (single-repo) — no registry.yaml.
    specify()
        .current_dir(tmp.path())
        .args(["init", "--schema-uri"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();
    assert!(!tmp.path().join(".specify/registry.yaml").exists());

    // Seed plan.yaml directly so the registry check is the first guard hit.
    fs::write(tmp.path().join(".specify/plan.yaml"), "name: demo\nchanges: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "merge"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("registry.yaml"), "diagnostic must reference registry.yaml, got: {msg}");
}

#[test]
fn workspace_merge_refuses_when_registry_empty() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Hub init writes an empty registry; seed a plan so the *empty
    // registry* path is the guard that fires.
    fs::write(tmp.path().join(".specify/plan.yaml"), "name: demo\nchanges: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "workspace", "merge"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "config");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("no projects"), "diagnostic must explain the empty registry, got: {msg}");
    assert!(msg.contains("registry add"), "diagnostic must hint at `registry add`, got: {msg}");
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
        .args(["init", "--schema-uri"])
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
        tmp.path().join(".specify/plan.yaml"),
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

    // Hand-write a registry too, so we can exercise stale-clone with
    // a deterministic fixture: a clone slot with a sync stamp whose
    // `url` disagrees with the registry.
    fs::create_dir_all(tmp.path().join(".specify")).unwrap();
    fs::write(
        tmp.path().join(".specify/registry.yaml"),
        "version: 1\n\
         projects:\n\
         \x20\x20- name: alpha\n\
         \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
         \x20\x20\x20\x20schema: omnia@v1\n",
    )
    .unwrap();
    let slot = tmp.path().join(".specify/workspace/alpha");
    fs::create_dir_all(slot.join(".git")).unwrap();
    fs::write(
        slot.join(".specify-sync.yaml"),
        "url: git@github.com:old/alpha.git\nschema: omnia@v1\n",
    )
    .unwrap();

    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "plan", "doctor"]).assert();
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
        tmp.path().join(".specify/plan.yaml"),
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

    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "plan", "doctor"]).assert();
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
        tmp.path().join(".specify/plan.yaml"),
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

    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
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
        .args(["--format", "json", "plan", "create", "demo"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "plan", "doctor"])
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
    let assert = specify().args(["plan", "doctor", "--help"]).assert().success();
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

// ---- specify initiative finalize (RFC-9 §4C) ----
//
// Wire-level integration tests for the precondition diagnostics. The
// happy-path classifier flow is covered by the in-process `MockProbe`
// against the orchestrator (see `cargo test --lib initiative_finalize`).
// The CLI tests below pin: (a) the failure-mode wire shape skill
// authors will rely on, and (b) the on-disk archive landing when no
// projects need probing.

#[test]
fn initiative_help_lists_finalize_subcommand() {
    let assert = specify().args(["initiative", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["create", "show", "finalize"] {
        assert!(
            stdout.contains(verb),
            "expected `initiative --help` to mention `{verb}`, got:\n{stdout}",
        );
    }
}

#[test]
fn initiative_finalize_help_documents_clean_and_dry_run() {
    let assert = specify().args(["initiative", "finalize", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for flag in ["--clean", "--dry-run"] {
        assert!(stdout.contains(flag), "expected --help to document `{flag}`, got:\n{stdout}");
    }
}

#[test]
fn initiative_finalize_refuses_when_plan_absent() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    assert!(
        !tmp.path().join(".specify/plan.yaml").exists(),
        "test precondition: plan.yaml must be absent",
    );

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "initiative", "finalize"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "plan-not-found");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("plan.yaml"), "msg should reference plan.yaml: {msg}");
    // Diagnostic should hint at the recovery sequence.
    assert!(
        msg.contains("plan create") || msg.contains("initiative create"),
        "msg should hint at plan/initiative create, got: {msg}",
    );
}

#[test]
fn initiative_finalize_refuses_on_non_terminal_entries() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Seed a plan with one done and one pending entry — pending is
    // not terminal for finalize.
    fs::write(
        tmp.path().join(".specify/plan.yaml"),
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
        .args(["--format", "json", "initiative", "finalize"])
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
    assert!(tmp.path().join(".specify/plan.yaml").exists(), "plan.yaml must be untouched");
}

#[test]
fn initiative_finalize_dry_run_archives_nothing_with_empty_registry() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Seed an all-terminal plan and rely on the hub-init's empty
    // registry — no per-project probes will run.
    fs::write(tmp.path().join(".specify/plan.yaml"), "name: foo\nchanges: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "initiative", "finalize", "--dry-run"])
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
    assert!(tmp.path().join(".specify/plan.yaml").exists(), "dry-run must not move plan.yaml");
}

#[test]
fn initiative_finalize_archives_when_all_terminal_and_no_registry() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join(".specify/plan.yaml"), "name: foo\nchanges: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "initiative", "finalize"])
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
    assert!(!tmp.path().join(".specify/plan.yaml").exists(), "plan.yaml must be archived");
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
fn initiative_finalize_idempotent_after_archive() {
    // Idempotency proof: the second `finalize` invocation after the
    // archive landed produces a clear `plan-not-found` refusal — the
    // canonical "initiative is already finalized" signal.
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join(".specify/plan.yaml"), "name: foo\nchanges: []\n").unwrap();

    // First run: archives the plan.
    specify().current_dir(tmp.path()).args(["initiative", "finalize"]).assert().success();
    assert!(!tmp.path().join(".specify/plan.yaml").exists());

    // Second run: plan is gone, refused with plan-not-found.
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "initiative", "finalize"])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["error"], "plan-not-found");
}
