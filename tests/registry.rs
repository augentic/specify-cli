//! Integration tests for `specify registry` (RFC-9 §2A).
//!
//! Covers `registry add`, `registry remove`, and the hub-mode
//! `registry validate` invariants surfaced after a fresh `init --hub`.
//! Schema-level `registry validate` coverage on non-hub projects lives
//! in `tests/change_umbrella.rs`.

use std::fs;

use tempfile::tempdir;

mod common;
use common::{init_hub, omnia_schema_dir, specify};

#[test]
fn init_hub_validate_succeeds_on_empty() {
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
    assert!(value["error"].is_null(), "success envelope must omit error: {value}");
}

#[test]
fn init_hub_validate_rejects_dot_url() {
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
         \x20\x20\x20\x20capability: hub\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(
        value["error"], "hub-cannot-be-project",
        "error must carry the stable diagnostic code, got: {value}"
    );
    let msg = value["message"].as_str().expect("message string");
    assert!(msg.contains("registry.yaml"), "message must scope the file: {msg}");
}

// ---- specify registry {add, remove} (RFC-9 §2A) ----

#[test]
fn registry_add_round_trips_through_show() {
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
            "--capability",
            "omnia@v1",
            "--description",
            "Alpha service",
        ])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["envelope-version"], 6);
    assert!(value["error"].is_null(), "success envelope must omit error: {value}");
    assert_eq!(value["added"]["name"], "alpha");
    assert_eq!(value["added"]["url"], "git@github.com:augentic/alpha.git");
    assert_eq!(value["added"]["capability"], "omnia@v1");
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
        .args([
            "--format",
            "json",
            "registry",
            "add",
            "self",
            "--url",
            ".",
            "--capability",
            "omnia@v1",
        ])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "hub-cannot-be-project");
    let msg = value["message"].as_str().expect("message");
    assert!(
        msg.contains("hub-cannot-be-project"),
        "diagnostic must carry the stable code, got: {msg}"
    );
}

#[test]
fn registry_add_rejects_non_kebab() {
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
            "--capability",
            "omnia@v1",
        ])
        .assert()
        .failure();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-add-name-not-kebab");
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
                "--capability",
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
    assert!(value["error"].is_null(), "success envelope must omit error: {value}");
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
fn registry_remove_warns_on_plan_ref() {
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
                "--capability",
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
    assert!(value["error"].is_null(), "success envelope must omit error: {value}");
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
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-remove-not-found");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("not found"), "msg: {msg}");
    assert!(msg.contains("ghost"), "msg: {msg}");
}

#[test]
fn registry_remove_refuses_when_absent() {
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
        serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-remove-no-registry");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("no registry declared"), "msg: {msg}");
}
