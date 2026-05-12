//! Integration tests for `specify init` (capability and `--hub` modes).
//!
//! Covers the on-disk shape produced by `init`, the JSON envelope, and
//! the clap-level invariants around the positional `<capability>`
//! argument and the `--hub` flag.

use std::fs;

use tempfile::tempdir;

mod common;
use common::{omnia_schema_dir, specify};

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

    assert_eq!(value["envelope-version"], 6);
    assert_eq!(value["capability-name"], "omnia");
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

    // Non-hub init writes only `project.yaml` and the `.specify/`
    // skeleton at the project root. Platform-component artefacts at the
    // repo root are operator-managed.
    for absent in ["registry.yaml", "plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "non-hub init must not pre-touch `{absent}` at the repo root"
        );
    }
}

#[test]
fn init_with_no_args_errors() {
    // Acceptance (c): `specify init` (no positional, no `--hub`) must
    // exit `2` (clap's parse-error slot) with clap's standard
    // "required arguments were not provided" diagnostic. The historical
    // post-parse `init-requires-capability-or-hub` diagnostic was lifted
    // into the clap surface (`required_unless_present = "hub"`).
    let tmp = tempdir().unwrap();
    let assert = specify().current_dir(tmp.path()).args(["init"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "clap parse errors map to exit code 2");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(
        stderr.contains("required arguments were not provided") && stderr.contains("CAPABILITY"),
        "diagnostic must surface clap's required-arg parse error, got stderr:\n{stderr}"
    );
    assert!(
        !tmp.path().join(".specify").exists(),
        "no .specify must be scaffolded on parse failure"
    );
}

#[test]
fn init_with_capability_and_hub_errors() {
    // Acceptance (d): `specify init <url> --hub` must exit `2` with
    // clap's "the argument cannot be used with" diagnostic. Same
    // motivation as `init_with_no_args_errors`: the invariant lives in
    // clap (`conflicts_with = "hub"`), not a post-parse diagnostic.
    let tmp = tempdir().unwrap();
    let assert = specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .arg("--hub")
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "clap parse errors map to exit code 2");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(
        stderr.contains("cannot be used with") && stderr.contains("--hub"),
        "diagnostic must mention the conflicts_with rule, got stderr:\n{stderr}"
    );
}

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
    assert_eq!(value["capability-name"], "hub");
    assert!(
        value["scaffolded-rule-keys"].as_array().expect("array").is_empty(),
        "hub init must not scaffold rule keys, got: {}",
        value["scaffolded-rule-keys"]
    );

    // Hub init scaffolds `project.yaml` (under `.specify/`) plus
    // `registry.yaml` at the repo root, and nothing else. `registry.yaml`
    // survives because bootstrapping a hub is bootstrapping its registry;
    // `change.md` and `plan.yaml` stay operator-managed.
    assert!(tmp.path().join(".specify/project.yaml").is_file());
    assert!(tmp.path().join("registry.yaml").is_file());
    for absent in ["plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "hub init must not pre-touch `{absent}` at the repo root"
        );
    }
    // Phase-pipeline directories MUST NOT be present.
    assert!(!tmp.path().join(".specify/slices").exists());
    assert!(!tmp.path().join(".specify/specs").exists());
    assert!(!tmp.path().join(".specify/.cache").exists());

    // project.yaml shape: `hub: true` only, no `capability:` field, and
    // no stale `schema:` sentinel.
    let project_yaml =
        fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read project.yaml");
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("schema:")),
        "hub project.yaml must omit the stale `schema:` field:\n{project_yaml}"
    );
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("capability:")),
        "hub project.yaml must omit the `capability:` field:\n{project_yaml}"
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

    // `change.md` is not scaffolded by hub init; it appears only after
    // the operator runs `specify change create <name>`.
}

#[test]
fn init_hub_refuses_when_present() {
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
