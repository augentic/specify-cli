//! Integration tests for `specify registry` (RFC-9 §2A).
//!
//! Covers `registry add`, `registry remove`, the hub-mode `registry
//! validate` invariants surfaced after a fresh `init --hub`, and the
//! schema-level `registry {show,validate}` matrix (RFC-3a C12/C13) on
//! single- and multi-project layouts.

use std::fs;

use serde_json::Value;
use tempfile::tempdir;

mod common;
use common::{Project, init_hub, omnia_schema_dir, parse_stderr, parse_stdout, specify};

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
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
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

    // Author a plan with one entry pointing at alpha. The merged
    // `change draft` verb scaffolds change.md and plan.yaml together.
    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "draft", "demo"])
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
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
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
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "registry-remove-no-registry");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("no registry declared"), "msg: {msg}");
}

// ---- Registry (RFC-3a C12) ----

#[test]
fn registry_load_from_tempdir() {
    use specify_domain::registry::Registry;

    let project = Project::init();
    let registry_path = project.root().join("registry.yaml");
    fs::write(
        &registry_path,
        "version: 1\n\
             projects:\n\
             \x20\x20- name: traffic\n\
             \x20\x20\x20\x20url: .\n\
             \x20\x20\x20\x20capability: omnia@v1\n",
    )
    .expect("write registry.yaml");

    let loaded =
        Registry::load(project.root()).expect("registry parses").expect("registry present");
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.projects.len(), 1);
    assert_eq!(loaded.projects[0].name, "traffic");
    assert_eq!(loaded.projects[0].url, ".");
    assert_eq!(loaded.projects[0].capability, "omnia@v1");
    assert!(loaded.is_single_repo());
}

// ---- Registry CLI verbs (RFC-3a C13) ----
//
// `specify registry {show, validate}` — dedicated verbs
// that isolate the same shape check the C12 hook drives through
// `specify plan validate`. The tests below cover the full
// matrix: absent / well-formed / malformed × show / validate ×
// text / json.

const REGISTRY_SINGLE: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    capability: omnia@v1
";

const REGISTRY_THREE: &str = "\
version: 1
projects:
  - name: monolith
    url: .
    capability: omnia@v1
    description: Core monolith service
  - name: orders
    url: ../orders
    capability: omnia@v1
    description: Order management service
  - name: payments
    url: git@github.com:org/payments.git
    capability: omnia@v1
    description: Payment processing service
";

fn write_registry(project: &Project, body: &str) {
    fs::write(project.root().join("registry.yaml"), body).expect("write registry");
}

#[test]
fn registry_show_absent() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "show"])
        .assert()
        .success();
    assert_eq!(assert.get_output().status.code(), Some(0));
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["registry"], Value::Null);
    let path = actual["path"].as_str().expect("path");
    assert!(
        path.ends_with("/registry.yaml"),
        "path should point at /registry.yaml at the repo root, got: {path}"
    );
}

#[test]
fn registry_show_valid() {
    let project = Project::init();
    write_registry(&project, REGISTRY_SINGLE);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "show"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    let registry = actual["registry"].as_object().expect("registry object");
    assert_eq!(registry["version"], 1);
    let projects = registry["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "traffic");
    assert_eq!(projects[0]["url"], ".");
    assert_eq!(projects[0]["capability"], "omnia@v1");
}

#[test]
fn registry_show_text_mode() {
    let project = Project::init();
    write_registry(&project, REGISTRY_SINGLE);

    let assert =
        specify().current_dir(project.root()).args(["registry", "show"]).assert().success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    for fragment in ["version: 1", "name: traffic", "url: .", "capability: omnia@v1"] {
        assert!(
            stdout.contains(fragment),
            "text show output should mention `{fragment}`, got:\n{stdout}"
        );
    }
}

#[test]
fn registry_show_malformed() {
    let project = Project::init();
    write_registry(&project, "version: 2\nprojects: []\n");

    let assert =
        specify().current_dir(project.root()).args(["registry", "show"]).assert().failure();
    assert_ne!(assert.get_output().status.code(), Some(0));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(
        stderr.contains("registry.yaml"),
        "stderr should mention registry.yaml, got: {stderr:?}"
    );
}

#[test]
fn registry_validate_absent() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .success();
    assert_eq!(assert.get_output().status.code(), Some(0));
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["registry"], Value::Null);
    assert!(actual["error"].is_null(), "success envelope must omit error: {actual}");

    let text =
        specify().current_dir(project.root()).args(["registry", "validate"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert!(
        stdout.contains("no registry declared"),
        "text validate should say 'no registry declared', got: {stdout:?}"
    );
}

#[test]
fn registry_validate_well_formed() {
    let project = Project::init();
    write_registry(&project, REGISTRY_SINGLE);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert!(actual["error"].is_null(), "success envelope must omit error: {actual}");
    let registry = actual["registry"].as_object().expect("registry object");
    assert_eq!(registry["version"], 1);
}

#[test]
fn registry_validate_multi_project() {
    let project = Project::init();
    write_registry(&project, REGISTRY_THREE);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert!(actual["error"].is_null(), "success envelope must omit error: {actual}");
    let projects = actual["registry"]["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 3);
}

#[test]
fn registry_validate_malformed_version() {
    let project = Project::init();
    write_registry(&project, "version: 2\nprojects: []\n");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    // Failure envelopes are written to stderr.
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "registry-version-unsupported");
    let msg = actual["message"].as_str().expect("message string");
    assert!(msg.contains("version"), "message should mention version, got: {msg}");
    assert!(msg.contains("registry.yaml"), "message should mention registry.yaml, got: {msg}");
}

#[test]
fn registry_validate_duplicate_name() {
    let project = Project::init();
    write_registry(
        &project,
        "\
version: 1
projects:
  - name: dup
    url: .
    capability: omnia@v1
  - name: dup
    url: ../other
    capability: omnia@v1
",
    );

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "registry", "validate"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    // Failure envelopes are written to stderr.
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "registry-project-name-duplicate");
    let msg = actual["message"].as_str().expect("message string");
    assert!(msg.contains("duplicate"), "message should mention duplicate, got: {msg}");
}

#[test]
fn registry_validate_non_kebab() {
    let project = Project::init();
    write_registry(
        &project,
        "\
version: 1
projects:
  - name: NotKebab
    url: .
    capability: omnia@v1
",
    );

    let assert =
        specify().current_dir(project.root()).args(["registry", "validate"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
}

#[test]
fn registry_validate_unknown_key() {
    let project = Project::init();
    write_registry(&project, "version: 1\nversions: 2\nprojects: []\n");

    let assert =
        specify().current_dir(project.root()).args(["registry", "validate"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
}

/// Plan "Done when" criterion: on a scaffolded project with no
/// registry, `specify registry validate` exits 0.
#[test]
fn registry_validate_on_bare_repo() {
    let project = Project::init();
    assert!(!project.root().join("registry.yaml").exists(), "bare repo must not have a registry");
    specify().current_dir(project.root()).args(["registry", "validate"]).assert().success();
}
