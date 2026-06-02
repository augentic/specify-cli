//! Integration tests for the `specrun slice` subcommand tree.
//!
//! Every test stands up a fresh `.specify/` project via `specrun init`,
//! drives `specrun slice *` through `assert_cmd`, and inspects both the
//! structured stdout (`--format json`) and the on-disk side effects the
//! verb is responsible for.
//!
//! Test style follows `tests/e2e.rs`: favour end-to-end execution of the
//! built binary over unit tests so the behaviour the skills consume is
//! the behaviour under test.

use std::fs;

mod common;
use common::{Project, parse_json, specrun};

// ---------------------------------------------------------------------------
// slice create
// ---------------------------------------------------------------------------

#[test]
fn create_writes_dir_and_metadata() {
    let project = Project::init();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    let dir = value["dir"].as_str().expect("dir string");
    assert!(dir.ends_with("/my-slice"), "dir should end with /my-slice, got: {dir}");
    assert_eq!(value["status"], "refining");
    let target = value["target"].as_str().expect("target string");
    assert!(target.starts_with("file://"));
    assert!(target.ends_with("/adapters/targets/omnia"));
    assert_eq!(value["created"], true);
    assert_eq!(value["restarted"], false);

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(slice_dir.is_dir(), "slice dir must exist");
    assert!(slice_dir.join("specs").is_dir(), "specs/ must exist");
    let meta = fs::read_to_string(slice_dir.join(".metadata.yaml")).expect("read metadata");
    assert!(meta.contains("status: refining"));
    assert!(meta.contains("file://") && meta.contains("targets/omnia"));
    assert!(meta.contains("created-at:"));
}

#[test]
fn create_rejects_uppercase_name() {
    let project = Project::init();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "BadName"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "invalid-name");
    assert!(
        value["message"].as_str().unwrap().contains("kebab-case")
            || value["message"].as_str().unwrap().contains("invalid name")
    );
}

#[test]
fn create_errors_on_collision() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-already-exists");
    assert!(value["message"].as_str().unwrap().contains("already exists"));
}

#[test]
fn create_continue_reuses_existing_dir() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "create", "my-slice", "--if-exists", "continue"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["created"], false);
    assert_eq!(value["restarted"], false);
}

// ---------------------------------------------------------------------------
// slice transition
// ---------------------------------------------------------------------------

#[test]
fn transition_walks_happy_path() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    for target in ["refined", "built"] {
        let assert = specrun()
            .current_dir(project.root())
            .args(["--format", "json", "slice", "transition", "my-slice", target])
            .assert()
            .success();
        let value = parse_json(&assert.get_output().stdout);
        assert_eq!(value["status"], target);
    }

    let meta = fs::read_to_string(project.slices_dir().join("my-slice").join(".metadata.yaml"))
        .expect("read metadata");
    assert!(meta.contains("status: built"));
    assert!(meta.contains("defined-at:"));
    assert!(meta.contains("completed-at:"));
}

#[test]
fn transition_rejects_illegal_edge() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    // Refining -> Built is not a legal edge (must pass through refined).
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "transition", "my-slice", "built"])
        .assert()
        .failure();
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "lifecycle");
}

#[test]
fn transition_rejects_merged_target() {
    // The `merged` lifecycle status is reserved for `slice merge run`,
    // which writes it atomically alongside the spec merge and archive
    // move. Hand-driven `slice transition <name> merged` would skip
    // that bookkeeping, so the dispatcher refuses the value with an
    // argument-error envelope (exit 2) before lifecycle ever runs.
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "transition", "my-slice", "merged"])
        .assert()
        .code(2);
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "argument");
    assert_eq!(value["exit-code"], 2);
    let message = value["message"].as_str().expect("message string");
    assert!(
        message.contains("specrun slice merge run"),
        "argument-error message must redirect to the merge runner; got:\n{message}"
    );
    assert!(
        message.contains("merged"),
        "argument-error message must name the rejected target; got:\n{message}"
    );
}

// ---------------------------------------------------------------------------
// slice touched-specs
// ---------------------------------------------------------------------------

#[test]
fn touched_specs_classifies_new_vs_modified() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");

    // Adapter `alpha` — no baseline, should classify as `new`.
    fs::create_dir_all(slice_dir.join("specs/alpha")).unwrap();
    fs::write(slice_dir.join("specs/alpha/spec.md"), "# Alpha\n").unwrap();

    // Adapter `beta` — baseline exists, should classify as `modified`.
    fs::create_dir_all(project.specs_dir().join("beta")).unwrap();
    fs::write(project.specs_dir().join("beta/spec.md"), "# Beta baseline\n").unwrap();
    fs::create_dir_all(slice_dir.join("specs/beta")).unwrap();
    fs::write(slice_dir.join("specs/beta/spec.md"), "# Beta delta\n").unwrap();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "touched-specs", "my-slice", "--scan"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["touched-specs"].as_array().expect("touched-specs array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[0]["type"], "new");
    assert_eq!(items[1]["name"], "beta");
    assert_eq!(items[1]["type"], "modified");

    // Scanning must have persisted the list into `.metadata.yaml`.
    let meta = fs::read_to_string(slice_dir.join(".metadata.yaml")).unwrap();
    assert!(meta.contains("touched-specs:"));
    assert!(meta.contains("name: alpha"));
    assert!(meta.contains("type: new"));
    assert!(meta.contains("name: beta"));
    assert!(meta.contains("type: modified"));
}

#[test]
fn touched_specs_accepts_explicit_list() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "touched-specs",
            "my-slice",
            "--set",
            "alpha:new,beta:modified",
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let items = value["touched-specs"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[1]["type"], "modified");
}

// ---------------------------------------------------------------------------
// slice overlap
// ---------------------------------------------------------------------------

#[test]
fn overlap_reports_shared_adapters() {
    let project = Project::init();
    // Two active slices both claim `login`.
    specrun().current_dir(project.root()).args(["slice", "create", "first"]).assert().success();
    specrun().current_dir(project.root()).args(["slice", "create", "second"]).assert().success();
    specrun()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "first", "--set", "login:new,oauth:new"])
        .assert()
        .success();
    specrun()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "second", "--set", "login:modified"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "overlap", "first"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let overlaps = value["overlaps"].as_array().unwrap();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0]["capability"], "login");
    assert_eq!(overlaps[0]["other-slice"], "second");
    assert_eq!(overlaps[0]["our-spec-type"], "new");
    assert_eq!(overlaps[0]["other-spec-type"], "modified");
}

#[test]
fn overlap_empty_for_disjoint_slices() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "alpha"]).assert().success();
    specrun().current_dir(project.root()).args(["slice", "create", "beta"]).assert().success();
    specrun()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "alpha", "--set", "aa:new"])
        .assert()
        .success();
    specrun()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "beta", "--set", "bb:new"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "overlap", "alpha"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["overlaps"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// slice drop
// ---------------------------------------------------------------------------

#[test]
fn drop_transitions_and_archives() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "slice",
            "drop",
            "my-slice",
            "--reason",
            "Needs design call-out",
        ])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["status"], "dropped");
    assert_eq!(value["drop-reason"], "Needs design call-out");
    let archive_path = value["archive-path"].as_str().unwrap();
    assert!(archive_path.ends_with("-my-slice"));

    // `.metadata.yaml` inside the archive should reflect the drop.
    let archived_meta = fs::read_to_string(format!("{archive_path}/.metadata.yaml")).unwrap();
    assert!(archived_meta.contains("status: dropped"));
    assert!(archived_meta.contains("drop-reason: Needs design call-out"));
    assert!(archived_meta.contains("dropped-at:"));
}

#[test]
fn metadata_without_outcome_still_parses() {
    use specify_workflow::slice::SliceMetadata;
    // Hand-craft a `.metadata.yaml` that predates the `outcome` field
    // and assert that SliceMetadata::load accepts it and leaves
    // `outcome` as None.
    let tmp = tempfile::tempdir().expect("tempdir");
    let slice_dir = tmp.path();
    let yaml = r#"target: omnia
status: refining
created-at: "2024-08-01T10:00:00Z"
"#;
    fs::write(slice_dir.join(".metadata.yaml"), yaml).expect("write metadata");
    let meta = SliceMetadata::load(slice_dir).expect("legacy metadata parses");
    assert!(
        meta.outcome.is_none(),
        "pre-existing metadata without an outcome field must load as None"
    );
}

#[test]
fn phase_outcome_round_trips_serde() {
    use specify_workflow::slice::Outcome;
    // Construction via struct literal would require crossing the
    // `#[non_exhaustive]` boundary on `Outcome`; round-trip through
    // YAML instead so the wire shape is what's exercised.
    for kind in ["success", "failure", "deferred"] {
        for phase in ["shape", "build", "merge"] {
            let yaml = format!(
                "phase: {phase}\noutcome: {kind}\nat: \"2024-08-01T10:00:00Z\"\nsummary: some summary\n"
            );
            let parsed: Outcome = serde_saphyr::from_str(&yaml).expect("parse");
            let reserialised = serde_saphyr::to_string(&parsed).expect("serialize");
            let reparsed: Outcome = serde_saphyr::from_str(&reserialised).expect("reparse");
            assert_eq!(parsed, reparsed, "round-trip failed for yaml:\n{yaml}");
        }
    }
}

// ---- Top-level help surfaces source/target axis verbs ----

#[test]
fn help_lists_axis_verbs() {
    let assert = specrun().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("slice"), "Top-level --help must still list `slice`, got:\n{stdout}");
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("source ")),
        "Top-level --help must list the `source` axis verb, got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("target ")),
        "Top-level --help must list the `target` axis verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("change ")),
        "Top-level --help must NOT list the retired `change` verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("adapter ")),
        "Top-level --help must NOT list the retired `adapter` verb, got:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// workflow §Requirement block contract — `slice validate` provenance gate
// ---------------------------------------------------------------------------

/// Stage a slice on disk and seed `<slice>/specs/login/spec.md`
/// directly, plus optionally a `plan.yaml` at the project root, so the
/// provenance gate inside `specrun slice validate` has both the spec
/// file and a plan-level source-bindings context to cross-validate
/// against. Returns the project handle so the caller can drive
/// `specrun slice validate` on it.
fn stage_slice_with_spec(spec_md: &str, plan_yaml: Option<&str>) -> Project {
    let project = Project::init().with_schemas();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let specs_dir = project.slices_dir().join("my-slice/specs/login");
    fs::create_dir_all(&specs_dir).expect("mkdir specs/login");
    fs::write(specs_dir.join("spec.md"), spec_md).expect("write spec.md");
    if let Some(yaml) = plan_yaml {
        project.seed_plan(yaml);
    }
    project
}

/// The validate surface now renders a `DiagnosticReport` on stdout and
/// fails payload-free: the per-rule discriminant lives in
/// `findings[].rule-id` on stdout, while stderr carries only the
/// payload-free `Error::Validation` envelope (exit 2). Assert the
/// expected `rule_id` appears in the rendered findings exactly.
fn assert_provenance_fail_rule(output: &std::process::Output, rule_id: &str) {
    let err = parse_json(&output.stderr);
    assert_eq!(err["exit-code"], 2);
    let report = parse_json(&output.stdout);
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|r| r["rule-id"] == rule_id),
        "expected rule_id `{rule_id}` in findings: {findings:#?}"
    );
}

/// Assert the rendered `DiagnosticReport` on stdout carries no finding
/// citing `rule_id`. Tolerates an empty stdout (e.g. a `--dump-model`
/// short-circuit) by treating it as "no findings".
fn assert_no_finding(output: &std::process::Output, rule_id: &str) {
    let report: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => return,
    };
    if let Some(findings) = report["findings"].as_array() {
        for finding in findings {
            assert_ne!(
                finding["rule-id"], rule_id,
                "no `{rule_id}` finding may appear; got: {findings:#?}"
            );
        }
    }
}

/// Locate the rendered diagnostic on stdout for `rule_id` and return
/// its operator-facing `impact` (the former `detail` row). Asserts exit
/// 2 along the way so callers can focus on the impact text.
fn find_finding_impact(output: &std::process::Output, rule_id: &str) -> String {
    let err = parse_json(&output.stderr);
    assert_eq!(err["exit-code"], 2);
    let report = parse_json(&output.stdout);
    let findings = report["findings"].as_array().expect("findings array");
    findings
        .iter()
        .find(|r| r["rule-id"] == rule_id)
        .and_then(|r| r["impact"].as_str())
        .unwrap_or_else(|| panic!("`{rule_id}` finding must be present in {findings:#?}"))
        .to_string()
}

const PLAN_WITH_LEGACY_MONOLITH: &str = "\
name: workflow-prov
lifecycle: pending
sources:
  legacy-monolith:
    adapter: code-typescript
    path: ./legacy
slices:
  - name: my-slice
    status: pending
    sources:
      - { source: legacy-monolith, lead: my-slice }
";

#[test]
fn validate_rejects_missing_id() {
    let spec = "### Requirement: Missing id\n\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n\n\
                body\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-id-missing");
}

#[test]
fn validate_rejects_malformed_id() {
    let spec = "### Requirement: Malformed id\n\n\
                ID: REQ-1\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-id-malformed");
}

#[test]
fn validate_rejects_missing_sources() {
    let spec = "### Requirement: No sources\n\n\
                ID: REQ-001\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-sources-missing");
}

#[test]
fn validate_rejects_missing_status() {
    let spec = "### Requirement: No status\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-status-missing");
}

#[test]
fn validate_rejects_unknown_status() {
    let spec = "### Requirement: Bogus status\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n\
                Status: maybe\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-status-unknown-value");
}

#[test]
fn validate_rejects_source_not_in_plan() {
    let spec = "### Requirement: Stray source key\n\n\
                ID: REQ-001\n\
                Sources: [phantom]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-source-undefined");
}

#[test]
fn validate_rejects_tag_status_mismatch() {
    let spec = "### Requirement: Lying tag [divergence]\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(assert.get_output(), "spec.requirement-tag-status-mismatch");
}

// ---------------------------------------------------------------------------
// Provenance projection — `slice provenance` (RFC-29c)
// ---------------------------------------------------------------------------

const CLEAN_SPEC_MD: &str = "### Requirement: Password reset request

ID: REQ-001
Sources: [legacy-monolith]
Status: agreed

The system lets a registered user request a password reset link by email.
";

/// Minimal projectable `model.yaml` for a slice named `my-slice` on the
/// earned core (`requirements` + `tasks`), with one fully-projected
/// requirement (kernel-owned fields present), so `slice provenance` can
/// reshape it into the audit view. `value` / `path` are no longer stored
/// here — the projection reads them from `evidence/<source>.yaml`.
const CLEAN_MODEL_YAML: &str = "version: 1
slice: my-slice
requirements:
  - id: REQ-001
    title: Password reset request
    status: agreed
    sources: [legacy-monolith]
    claims:
      - source: legacy-monolith
        id: password-reset.request
        kind: requirement
    statement: The system lets a registered user request a password reset link by email.
tasks: []
";

/// Evidence the provenance projection reads `value` / `path` and
/// document-level `authority` from when reshaping `CLEAN_MODEL_YAML`.
const CLEAN_EVIDENCE_YAML: &str = "authority: behaviour
lead: my-slice
claims:
  - id: password-reset.request
    kind: requirement
    statement: \"Password reset request returns a 200 response.\"
    path: src/users/reset.ts#L42
";

#[test]
fn provenance_projects_from_model() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("model.yaml"), CLEAN_MODEL_YAML).expect("write model.yaml");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), CLEAN_EVIDENCE_YAML)
        .expect("write evidence");
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "provenance", "my-slice"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("REQ-001"), "projection should list REQ-001, got: {stdout}");
    assert!(
        stdout.contains("single-source"),
        "projection should carry the resolution, got: {stdout}"
    );
}

#[test]
fn provenance_fails_without_model() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "provenance", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
}

// ---------------------------------------------------------------------------
// Model viewer — `slice model show` (RFC-29 §"Operator surface")
// ---------------------------------------------------------------------------

#[test]
fn model_show_renders_json_and_text() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("model.yaml"), CLEAN_MODEL_YAML).expect("write model.yaml");

    // `--format json` serialises the persisted model verbatim.
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["slice"], "my-slice");
    assert_eq!(value["requirements"][0]["id"], "REQ-001");
    assert_eq!(value["requirements"][0]["title"], "Password reset request");

    // Text mode prints the concise human view.
    let assert = specrun()
        .current_dir(project.root())
        .args(["slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("slice: my-slice"), "header must name the slice, got: {stdout}");
    assert!(
        stdout.contains("REQ-001 [agreed] Password reset request"),
        "requirement line must render id/status/title, got: {stdout}"
    );
    assert!(
        stdout.contains("sources: legacy-monolith"),
        "requirement line must render sources, got: {stdout}"
    );
}

#[test]
fn model_show_fails_without_model() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-model-missing");
}

// ---------------------------------------------------------------------------
// RFC-35 D8 — `slice validate` spec file-location gate
// ---------------------------------------------------------------------------

#[test]
fn validate_emits_file_location_when_root_spec_md_exists_but_no_canonical_specs() {
    let project = Project::init().with_schemas();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("spec.md"), CLEAN_SPEC_MD).expect("write root spec.md");
    fs::remove_dir_all(slice_dir.join("specs")).expect("remove specs dir created by slice create");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-pre-adapter-gate");
    let detail = find_finding_impact(assert.get_output(), "specs.file-location");
    assert!(
        detail.contains("specs/<unit>/spec.md"),
        "detail must name the canonical layout, got: {detail}"
    );
    assert!(detail.contains("slice root"), "detail must mention the slice root, got: {detail}");
}

#[test]
fn validate_does_not_emit_file_location_when_canonical_specs_exist() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("spec.md"), "stale root copy").expect("write root spec.md");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let stderr = assert.get_output().stderr.clone();
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stderr)
        && let Some(results) = value["results"].as_array()
    {
        for r in results {
            let rule_id = r["rule-id"].as_str().unwrap_or("");
            assert_ne!(
                rule_id, "specs.file-location",
                "file-location gate must not fire when canonical specs exist"
            );
        }
    }
}

#[test]
fn validate_does_not_emit_file_location_when_no_root_spec_md() {
    let project = Project::init().with_schemas();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let stderr = assert.get_output().stderr.clone();
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stderr)
        && let Some(results) = value["results"].as_array()
    {
        for r in results {
            let rule_id = r["rule-id"].as_str().unwrap_or("");
            assert_ne!(
                rule_id, "specs.file-location",
                "file-location gate must not fire when no root spec.md exists"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// component catalog contract — `slice validate` catalog drift gate
// ---------------------------------------------------------------------------

/// Evidence with a `component:` directive on a claim.
const EVIDENCE_WITH_COMPONENT: &str = "authority: behaviour
lead: my-slice
claims:
  - kind: region
    id: task-list-footer
    component: tab-bar
    statement: \"Bottom tab bar with three tabs.\"
";

/// Evidence with `notes.candidate_component` (informational hint,
/// not a hard `component:` directive).
const EVIDENCE_WITH_CANDIDATE_COMPONENT: &str = "authority: behaviour
lead: my-slice
claims:
  - kind: region
    id: task-list-header
    notes:
      candidate_component: hero-banner
    statement: \"Hero banner at top of screen.\"
";

/// A minimal catalog YAML with one confirmed and one rejected entry.
const CATALOG_YAML: &str = "version: 1
components:
  tab-bar:
    status: confirmed
    description: \"Bottom navigation across the primary app sections.\"
  hero-banner:
    status: rejected
    description: \"Not a real shared component.\"
";

/// Plan that declares a `ui-screens` source for the `my-slice` entry.
const PLAN_WITH_UI_SCREENS: &str = "\
name: component-catalog
lifecycle: pending
sources:
  ui-screens:
    adapter: screenshots
    path: ./screens
slices:
  - name: my-slice
    status: pending
    sources:
      - { source: ui-screens, lead: my-slice }
";

/// Stage a slice with Evidence containing `component:` directives
/// and optionally a component catalog.
fn stage_slice_with_catalog(evidence: &str, catalog: Option<&str>, plan: Option<&str>) -> Project {
    let project = Project::init().with_schemas();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("ui-screens.yaml"), evidence).expect("write evidence");

    if let Some(cat) = catalog {
        let catalog_dir = project.root().join(".specify/design-system");
        fs::create_dir_all(&catalog_dir).expect("mkdir design-system");
        fs::write(catalog_dir.join("components.yaml"), cat).expect("write catalog");
    }

    if let Some(yaml) = plan {
        project.seed_plan(yaml);
    }
    project
}

#[test]
fn validate_skips_catalog_drift_without_catalog() {
    let project =
        stage_slice_with_catalog(EVIDENCE_WITH_COMPONENT, None, Some(PLAN_WITH_UI_SCREENS));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    assert_no_finding(assert.get_output(), "slice-catalog-drift");
}

#[test]
fn validate_passes_when_slug_confirmed() {
    let project = stage_slice_with_catalog(
        EVIDENCE_WITH_COMPONENT,
        Some(CATALOG_YAML),
        Some(PLAN_WITH_UI_SCREENS),
    );
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    assert_no_finding(assert.get_output(), "slice-catalog-drift");
}

#[test]
fn validate_detects_missing_catalog_entry() {
    let catalog_without_tab_bar = "version: 1\ncomponents:\n  card-row:\n    status: confirmed\n";
    let project = stage_slice_with_catalog(
        EVIDENCE_WITH_COMPONENT,
        Some(catalog_without_tab_bar),
        Some(PLAN_WITH_UI_SCREENS),
    );
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let detail = find_finding_impact(assert.get_output(), "slice-catalog-drift");
    assert!(
        detail.contains("tab-bar") && detail.contains("no entry exists"),
        "drift detail should name the missing slug, got: {detail}"
    );
}

#[test]
fn validate_detects_rejected_catalog_entry() {
    let catalog_with_rejected = "version: 1\ncomponents:\n  tab-bar:\n    status: rejected\n";
    let project = stage_slice_with_catalog(
        EVIDENCE_WITH_COMPONENT,
        Some(catalog_with_rejected),
        Some(PLAN_WITH_UI_SCREENS),
    );
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let detail = find_finding_impact(assert.get_output(), "slice-catalog-drift");
    assert!(
        detail.contains("tab-bar") && detail.contains("rejected"),
        "drift detail should describe the rejected status, got: {detail}"
    );
}

#[test]
fn validate_ignores_candidate_notes() {
    let project = stage_slice_with_catalog(
        EVIDENCE_WITH_CANDIDATE_COMPONENT,
        Some(CATALOG_YAML),
        Some(PLAN_WITH_UI_SCREENS),
    );
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    assert_no_finding(assert.get_output(), "slice-catalog-drift");
}

#[test]
fn validate_passes_with_empty_catalog() {
    let empty_catalog = "version: 1\ncomponents: {}\n";
    let evidence_no_component = "authority: behaviour
lead: my-slice
claims:
  - kind: region
    id: task-list-body
    statement: \"Main task list body.\"
";
    let project = stage_slice_with_catalog(
        evidence_no_component,
        Some(empty_catalog),
        Some(PLAN_WITH_UI_SCREENS),
    );
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    assert_no_finding(assert.get_output(), "slice-catalog-drift");
}

#[test]
fn validate_skips_provenance_without_metadata() {
    // pre-2.0 (or pre-synthesis) state. The provenance gate must
    // not fire and the slice progresses to the existing adapter rule
    // run. The adapter rules will still surface deferred /
    // pass-style results — we only assert the provenance rule ids
    // are NOT present.
    let spec = "### Requirement: pre-2.0 body\n\n\
                ID: REQ-001\n\n\
                body that has no Sources or Status yet\n";
    let project = stage_slice_with_spec(spec, None);
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    // Whether the run passes or fails (existing adapter rules may
    // still produce findings on the synthetic slice), no provenance
    // rule should appear on the rendered report.
    if let Ok(report) = serde_json::from_slice::<serde_json::Value>(&assert.get_output().stdout)
        && let Some(findings) = report["findings"].as_array()
    {
        for finding in findings {
            let rule_id = finding["rule-id"].as_str().unwrap_or("");
            assert!(
                !rule_id.starts_with("spec.requirement-"),
                "no provenance rule should fire on a pre-2.0 spec.md, got: {rule_id}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// slice validate — `discovery-lead-synopsis-thin` advisory (DECISIONS §Lead reconciliation D2.1)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Slice synthesis engine — `slice synthesize` (RFC-29c M2b)
// ---------------------------------------------------------------------------

/// Evidence the synthesis kernel resolves authority and anchors claims
/// against. One `requirement` claim, behaviour authority.
const SYNTH_EVIDENCE_YAML: &str = "authority: behaviour
lead: my-slice
claims:
  - id: password-reset.request
    kind: requirement
    statement: \"The system lets a user request a reset link.\"
    path: src/users/reset.ts#L42
";

/// Agent synthesis response — one agreed requirement (single claim) and
/// one task. Kernel-owned fields omitted so the kernel projects them.
const SYNTH_RESPONSE_JSON: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Request password reset",
        "unit": "password-reset",
        "claims": [
          { "source": "legacy-monolith", "id": "password-reset.request", "kind": "requirement" }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// Stage a slice with one bound source's Evidence plus a plan entry, so
/// `slice synthesize` can read both the inline Evidence (dry-run) and
/// the on-disk Evidence the kernel resolves authority from (`--from`).
fn stage_synthesizable_slice() -> Project {
    let project = Project::init().with_schemas();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), SYNTH_EVIDENCE_YAML)
        .expect("write evidence");
    project.seed_plan(PLAN_WITH_LEGACY_MONOLITH);
    project
}

#[test]
fn synthesize_dry_run_emits_inputs_envelope() {
    let project = stage_synthesizable_slice();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--dry-run"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["kind"], "inputs");
    assert_eq!(value["slice"], "my-slice");
    let sources = value["sources"].as_array().expect("sources array");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["source"], "legacy-monolith");
    assert_eq!(sources[0]["lead"], "my-slice");
    assert!(
        !sources[0]["claims"].as_array().expect("claims array").is_empty(),
        "inline Evidence claims must be carried into the envelope"
    );
    assert!(
        !value["shape-brief"].as_str().expect("shape-brief string").is_empty(),
        "the resolved target shape brief must be embedded"
    );

    // Dry-run writes nothing.
    assert!(
        !project.slices_dir().join("my-slice/model.yaml").exists(),
        "dry-run must not write model.yaml"
    );

    // The always-agent / cache: opt-out signal fires on the dry-run.
    let journal =
        fs::read_to_string(project.root().join(".specify/journal.jsonl")).expect("read journal");
    assert!(
        journal.contains("slice.synthesize.agent"),
        "dry-run must emit slice.synthesize.agent, got:\n{journal}"
    );
}

#[test]
fn synthesize_from_projects_and_persists() {
    let project = stage_synthesizable_slice();
    let response_path = project.root().join("response.json");
    fs::write(&response_path, SYNTH_RESPONSE_JSON).expect("write response");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--from"])
        .arg(&response_path)
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["slice"], "my-slice");
    let artifacts: Vec<String> = value["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_string())
        .collect();
    for expected in
        ["proposal.md", "specs/password-reset/spec.md", "design.md", "tasks.md", "model.yaml"]
    {
        assert!(artifacts.contains(&expected.to_string()), "missing {expected} in {artifacts:?}");
    }

    let slice_dir = project.slices_dir().join("my-slice");
    for rel in
        ["proposal.md", "design.md", "tasks.md", "model.yaml", "specs/password-reset/spec.md"]
    {
        assert!(slice_dir.join(rel).is_file(), "{rel} must be persisted");
    }

    // The persisted model.yaml is schema-valid: `slice model show`
    // loads it through `SliceModel::parse_yaml`, which schema-gates.
    let show = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let model = parse_json(&show.get_output().stdout);
    assert_eq!(model["slice"], "my-slice");
    assert_eq!(model["requirements"][0]["id"], "REQ-001");
    assert_eq!(model["requirements"][0]["status"], "agreed");
    assert_eq!(model["requirements"][0]["sources"][0], "legacy-monolith");

    // spec.md carries the kernel-rendered provenance lines.
    let spec = fs::read_to_string(slice_dir.join("specs/password-reset/spec.md")).expect("spec.md");
    assert!(spec.contains("ID: REQ-001"), "spec.md must carry the projected ID, got:\n{spec}");
    assert!(spec.contains("Sources: legacy-monolith"), "spec.md must carry Sources, got:\n{spec}");
    assert!(spec.contains("Status: agreed"), "spec.md must carry Status, got:\n{spec}");

    // The paired started/completed journal events bracket the write.
    let journal =
        fs::read_to_string(project.root().join(".specify/journal.jsonl")).expect("read journal");
    assert!(journal.contains("slice.synthesize.started"), "missing started, got:\n{journal}");
    assert!(journal.contains("slice.synthesize.completed"), "missing completed, got:\n{journal}");
}

#[test]
fn synthesize_requires_a_mode() {
    let project = stage_synthesizable_slice();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-synthesize-mode-required");
}

// ---------------------------------------------------------------------------
// Slice synthesis engine — acceptance / end-to-end coverage (RFC-29c C12)
//
// The kernel-level cases (normalize, orphan, divergence, determinism)
// are unit-covered in `crates/workflow/src/slice/synthesis/*`; these
// drive the same paths end-to-end through the built `slice synthesize`
// command so the behaviour the `/spec:refine` skill consumes is the
// behaviour under test. The drift-validator surface is owned by
// `tests/slice_drift.rs`; here we only add the synthesized-slice happy
// path it does not exercise.
// ---------------------------------------------------------------------------

/// Write `response_json` to `<root>/response.json` and run
/// `slice synthesize my-slice --from response.json`, returning the
/// process output for the caller to assert on.
fn run_synthesize_from(project: &Project, response_json: &str) -> std::process::Output {
    let response_path = project.root().join("response.json");
    fs::write(&response_path, response_json).expect("write response");
    specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--from"])
        .arg(&response_path)
        .assert()
        .get_output()
        .clone()
}

/// A response that pre-assigns every kernel-owned field to a wrong (but
/// schema-valid) value — `REQ-999`, `status: conflict`, a stray
/// `sources` list, a claim `winner`, and a bogus `model.slice` /
/// `model.project` header. The kernel must ignore each and re-derive the
/// canonical projection (RFC-29c §"Synthesis response": normalize, never
/// reject). The single in-Evidence claim is `agreed` once re-derived.
const SYNTH_RESPONSE_PRE_ASSIGNED: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "slice": "bogus-slice",
    "project": "bogus-project",
    "requirements": [
      {
        "id": "REQ-999",
        "title": "Request password reset",
        "status": "conflict",
        "unit": "password-reset",
        "sources": ["wrong-source"],
        "claims": [
          { "source": "legacy-monolith", "id": "password-reset.request", "kind": "requirement", "winner": true }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// A response whose claim cites an Evidence id (`ghost-claim`) absent
/// from `evidence/legacy-monolith.yaml` — the kernel cannot anchor it
/// and aborts `slice-model-source-orphan`.
const SYNTH_RESPONSE_ORPHAN: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Request password reset",
        "unit": "password-reset",
        "claims": [
          { "source": "legacy-monolith", "id": "ghost-claim", "kind": "requirement" }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// A response whose claim records `kind: criterion`, but the matching
/// Evidence claim `password-reset.request` is recorded as a
/// `requirement` — the kernel aborts `slice-model-claim-kind-mismatch`.
const SYNTH_RESPONSE_KIND_MISMATCH: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Request password reset",
        "unit": "password-reset",
        "claims": [
          { "source": "legacy-monolith", "id": "password-reset.request", "kind": "criterion" }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// Plan binding two sources to `my-slice`: documentation-authority
/// `docs` and behaviour-authority `legacy`, both citing the same
/// `password-reset.expiry` claim. The RFC-29c §"Slice model (D4)"
/// worked divergence: the documentation `criterion` beats the behaviour
/// `example`.
const DIVERGENCE_PLAN: &str = "\
name: divergence
lifecycle: pending
sources:
  docs:
    adapter: documentation
    path: ./docs
  legacy:
    adapter: code-typescript
    path: ./legacy
slices:
  - name: my-slice
    status: pending
    project: test-proj
    sources:
      - { source: docs, lead: my-slice }
      - { source: legacy, lead: my-slice }
";

/// Documentation-authority Evidence: the criterion claim that wins the
/// divergence. The provenance projection reads its `value` / `path`.
const DIVERGENCE_EVIDENCE_DOCS: &str = "authority: documentation
lead: my-slice
claims:
  - id: password-reset.expiry
    kind: criterion
    criterion: Reset links expire after 30 minutes.
    path: docs/identity/reset.md#L7
";

/// Behaviour-authority Evidence: the example claim that loses the
/// divergence but survives in provenance with `winner: false`.
const DIVERGENCE_EVIDENCE_LEGACY: &str = "authority: behaviour
lead: my-slice
claims:
  - id: password-reset.expiry
    kind: example
    output: expiresAt = createdAt + 24h
    path: src/users/reset.ts#L88
";

/// Agent response for the divergence slice — one `disagreed`
/// requirement citing both sources' `password-reset.expiry` claim.
const DIVERGENCE_RESPONSE_JSON: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Reset link expiry",
        "unit": "password-reset",
        "agreement": "disagreed",
        "claims": [
          { "source": "docs", "id": "password-reset.expiry", "kind": "criterion" },
          { "source": "legacy", "id": "password-reset.expiry", "kind": "example" }
        ],
        "statement": "Reset links expire after 30 minutes."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Enforce reset link expiry.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Reset expiry\nWhy this slice exists.\n",
    "design": "# Design\nExpiry handling.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Reset link expiry\nLinks expire after 30 minutes.\n" }
    ]
  }
}
"###;

/// Stage `my-slice` with two bound sources (docs + legacy) sharing the
/// `password-reset.expiry` claim, so the kernel resolves a per-kind
/// divergence.
fn stage_divergence_slice() -> Project {
    let project = Project::init().with_schemas();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("docs.yaml"), DIVERGENCE_EVIDENCE_DOCS).expect("write docs");
    fs::write(evidence_dir.join("legacy.yaml"), DIVERGENCE_EVIDENCE_LEGACY).expect("write legacy");
    project.seed_plan(DIVERGENCE_PLAN);
    project
}

#[test]
fn synthesize_dry_run_omits_authority() {
    // The inputs envelope carries each source's inline `lead` + `claims`
    // and the resolved shape brief, but never the document-level
    // `authority` — the kernel resolves authority post-response (RFC-29c
    // §"Synthesis response").
    let project = stage_synthesizable_slice();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--dry-run"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.clone();

    let value = parse_json(&stdout);
    assert_eq!(value["sources"][0]["lead"], "my-slice");
    assert!(
        !value["sources"][0]["claims"].as_array().expect("claims array").is_empty(),
        "inline Evidence claims must be carried"
    );
    assert!(!value["shape-brief"].as_str().expect("shape-brief").is_empty());

    // No `authority` key anywhere in the rendered envelope.
    let text = String::from_utf8(stdout).expect("utf8 stdout");
    assert!(
        !text.contains("authority"),
        "authority must be absent from the inputs envelope: {text}"
    );
}

#[test]
fn synthesize_from_writes_no_provenance_file() {
    // RFC-29c §"Command": provenance is carried inline in `model.yaml`;
    // there is no persisted `provenance.yaml`.
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "synthesize --from must succeed");

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(slice_dir.join("model.yaml").is_file(), "model.yaml must be persisted");
    assert!(
        !slice_dir.join("provenance.yaml").exists(),
        "synthesize must never write a provenance.yaml"
    );
}

#[test]
fn synthesize_normalizes_pre_assigned_fields() {
    // The agent pre-assigns wrong-but-valid kernel/header fields; the
    // command ignores them all and persists the canonical derivation
    // (RFC-29c §"Synthesis response": normalize, never reject).
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_PRE_ASSIGNED);
    assert_eq!(output.status.code(), Some(0), "a normalizing projection must succeed");

    let show = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let model = parse_json(&show.get_output().stdout);

    // Header re-stamped from the slice, not the agent's bogus values.
    assert_eq!(model["slice"], "my-slice");
    assert!(model.get("project").is_none() || model["project"].is_null());

    // Requirement fields re-derived: REQ-001 (not REQ-999), agreed (not
    // conflict), sources [legacy-monolith] (not wrong-source), and no
    // winner marker on the single agreed claim.
    let req = &model["requirements"][0];
    assert_eq!(req["id"], "REQ-001");
    assert_eq!(req["status"], "agreed");
    assert_eq!(req["sources"][0], "legacy-monolith");
    assert_eq!(req["sources"].as_array().expect("sources array").len(), 1);
    assert!(
        req["claims"][0].get("winner").is_none() || req["claims"][0]["winner"].is_null(),
        "an agreed single-claim requirement carries no winner marker"
    );
}

#[test]
fn synthesize_aborts_on_source_orphan() {
    // A claim that anchors no on-disk Evidence aborts the command before
    // any write, emitting the failure journal event (RFC-29c §"Persist
    // pipeline" step 1).
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_ORPHAN);
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output.stderr);
    assert_eq!(value["error"], "slice-model-source-orphan");

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(!slice_dir.join("model.yaml").exists(), "an aborted synthesis writes nothing");

    let journal =
        fs::read_to_string(project.root().join(".specify/journal.jsonl")).expect("read journal");
    assert!(journal.contains("slice.synthesize.failed"), "abort must emit failed, got:\n{journal}");
    assert!(
        !journal.contains("slice.synthesize.completed"),
        "an aborted synthesis must not emit completed, got:\n{journal}"
    );
}

#[test]
fn synthesize_aborts_on_claim_kind_mismatch() {
    // A claim kind that disagrees with the kind Evidence records for the
    // same `(source, id)` aborts `slice-model-claim-kind-mismatch` (D13).
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_KIND_MISMATCH);
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output.stderr);
    assert_eq!(value["error"], "slice-model-claim-kind-mismatch");

    assert!(
        !project.slices_dir().join("my-slice/model.yaml").exists(),
        "an aborted synthesis writes nothing"
    );
}

#[test]
fn synthesize_resolves_per_kind_divergence() {
    // The RFC-29c worked divergence: a documentation `criterion` beats a
    // behaviour `example`. The command derives `status: divergence`, the
    // winner / loser markers, the rendered source order, and the
    // `[divergence]` spec tag.
    let project = stage_divergence_slice();
    let output = run_synthesize_from(&project, DIVERGENCE_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "the divergence slice synthesizes");

    let show = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let model = parse_json(&show.get_output().stdout);
    let req = &model["requirements"][0];
    assert_eq!(req["id"], "REQ-001");
    assert_eq!(req["status"], "divergence");
    // Documentation (docs) outranks behaviour (legacy), so docs renders
    // first and wins; legacy loses.
    assert_eq!(req["sources"][0], "docs");
    assert_eq!(req["sources"][1], "legacy");
    assert_eq!(req["claims"][0]["source"], "docs");
    assert_eq!(req["claims"][0]["winner"], true);
    assert_eq!(req["claims"][1]["source"], "legacy");
    assert_eq!(req["claims"][1]["winner"], false);

    // spec.md carries the `[divergence]` heading tag and the matching
    // Status line.
    let spec =
        fs::read_to_string(project.slices_dir().join("my-slice/specs/password-reset/spec.md"))
            .expect("spec.md");
    assert!(
        spec.contains("[divergence]"),
        "non-agreed status renders the heading tag, got:\n{spec}"
    );
    assert!(spec.contains("Status: divergence"), "spec.md must carry the projected status");
    assert!(spec.contains("Sources: docs, legacy"), "spec.md renders the ordered source list");
}

#[test]
fn synthesize_then_validate_is_drift_clean() {
    // A slice synthesized by the command must pass `slice validate`'s
    // typed-model drift gate: the command loaded and re-validated
    // `model.yaml`, so none of the seven RFC-29c §"Drift validation"
    // findings fire. (Crafted-bad-slice coverage lives in
    // `tests/slice_drift.rs`; this is the synthesized happy path.)
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "synthesize must succeed before validate");

    let validate = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let output = validate.get_output();
    for rule_id in [
        "slice-model-schema",
        "slice-spec-provenance-stale",
        "slice-model-target-drift",
        "slice-model-source-orphan",
        "slice-model-cross-ref-orphan",
        "slice-model-claim-kind-mismatch",
        "slice-model-id-grammar",
    ] {
        assert_no_finding(output, rule_id);
    }
}

#[test]
fn synthesize_then_provenance_recomputes_labels() {
    // `slice provenance` over a synthesized divergence model recomputes
    // the `authority-resolved` label and reads each claim's `value` /
    // `path` from on-disk Evidence (RFC-29c §"Provenance projection").
    let project = stage_divergence_slice();
    let output = run_synthesize_from(&project, DIVERGENCE_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "the divergence slice synthesizes");

    let prov = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "provenance", "my-slice"])
        .assert()
        .success();
    let index = parse_json(&prov.get_output().stdout);
    let req = &index["requirements"][0];
    assert_eq!(req["id"], "REQ-001");
    assert_eq!(req["status"], "divergence");
    // Recomputed, not read from the model.
    assert_eq!(req["resolution"], "authority-resolved");
    assert_eq!(req["resolution-trace"]["step"], "default-authority-ordering");
    assert_eq!(req["resolution-trace"]["winner"], "docs");

    // `value` / `path` are read from Evidence for both the winner and
    // the dropped loser.
    let claims = req["contributing-claims"].as_array().expect("contributing-claims array");
    let docs = claims.iter().find(|c| c["source"] == "docs").expect("docs claim");
    assert_eq!(docs["value"], "Reset links expire after 30 minutes.");
    assert_eq!(docs["path"], "docs/identity/reset.md#L7");
    assert_eq!(docs["winner"], true);
    let legacy = claims.iter().find(|c| c["source"] == "legacy").expect("legacy claim");
    assert_eq!(legacy["value"], "expiresAt = createdAt + 24h");
    assert_eq!(legacy["path"], "src/users/reset.ts#L88");
    assert_eq!(legacy["winner"], false);
}

#[test]
fn synthesize_from_is_deterministic() {
    // RFC-29c §"Kernel determinism": running `--from` twice over the
    // same response yields a byte-identical `model.yaml`. (The model
    // carries no timestamp, and the kernel is target-independent.)
    let project = stage_synthesizable_slice();
    let model_path = project.slices_dir().join("my-slice/model.yaml");

    assert_eq!(run_synthesize_from(&project, SYNTH_RESPONSE_JSON).status.code(), Some(0));
    let first = fs::read_to_string(&model_path).expect("first model.yaml");

    assert_eq!(run_synthesize_from(&project, SYNTH_RESPONSE_JSON).status.code(), Some(0));
    let second = fs::read_to_string(&model_path).expect("second model.yaml");

    assert_eq!(first, second, "model.yaml must be byte-identical across two synthesis runs");
}

#[test]
fn validate_flags_thin_synopsis_non_blocking() {
    // A thin same-slug synopsis the agent cannot match or split on,
    // alongside a content-bearing one. The advisory must surface at
    // `suggestion` severity (non-blocking by the shared
    // `blocking_present` predicate — only `critical`/`important`
    // violations gate exit), nudging without parking the slice. Only
    // the thin `docs:identity-api` lead is flagged; the content-bearing
    // `legacy:identity-api` lead is not. (Adapter validation still
    // surfaces unrelated findings on this synthetic slice, so the test
    // asserts on the advisory finding itself rather than the overall
    // exit code — matching the suite's `assert_no_finding` convention.)
    let project = Project::init();
    specrun().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let discovery = "\
# Discovery — identity

## Lead inventory

### docs:identity-api

- lead: identity-api
- source: docs
- synopsis: Identity API.

### legacy:identity-api

- lead: identity-api
- source: legacy
- synopsis: Authentication and account-access API covering login, token refresh, and profile reads.
";
    fs::write(project.root().join("discovery.md"), discovery).expect("write discovery.md");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let report = parse_json(&assert.get_output().stdout);
    let findings = report["findings"].as_array().expect("findings array");
    let thin: Vec<_> =
        findings.iter().filter(|f| f["rule-id"] == "discovery-lead-synopsis-thin").collect();
    assert_eq!(
        thin.len(),
        1,
        "exactly one thin-synopsis finding expected (only the `docs:identity-api` lead), got: \
         {findings:#?}"
    );
    let impact = thin[0]["impact"].as_str().unwrap_or_default();
    assert!(impact.contains("docs:identity-api"), "finding must name the thin lead, got: {impact}");
    let severity = thin[0]["severity"].as_str().unwrap_or_default();
    assert_eq!(
        severity, "suggestion",
        "advisory finding must be `suggestion` severity so it never blocks"
    );
}
