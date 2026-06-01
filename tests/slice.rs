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
// `provenance.yaml` audit index — `slice validate` provenance drift gate
// ---------------------------------------------------------------------------

/// Minimal provenance.yaml for a slice named `my-slice` with one
/// requirement `REQ-001` whose single contributing claim cites
/// `legacy-monolith :: REQ-001` (the same id we'll seed the evidence
/// file with by default).
const CLEAN_RECONCILIATION_YAML: &str = "version: 1
slice: my-slice
generated-at: 2026-05-22T13:15:00Z
generator: specify@2.1.0
requirements:
  - id: REQ-001
    status: agreed
    sources: [legacy-monolith]
    contributing-claims:
      - source: legacy-monolith
        id: password-reset.request
        kind: requirement
        value: \"Password reset request returns a 200 response.\"
        path: src/users/reset.ts#L42
    resolution: single-source
";

const CLEAN_SPEC_MD: &str = "### Requirement: Password reset request

ID: REQ-001
Sources: [legacy-monolith]
Status: agreed

The system lets a registered user request a password reset link by email.
";

const CLEAN_EVIDENCE_YAML: &str = "authority: behaviour
lead: my-slice
claims:
  - kind: requirement
    id: password-reset.request
    statement: \"Password reset request returns a 200 response.\"
    path: src/users/reset.ts#L42
";

/// Stage a fully-wired slice with provenance.yaml + spec.md + evidence
/// so the drift gate has every input it needs and the baseline test
/// fixture validates clean. Caller may then mutate any file before
/// re-running `slice validate` to exercise drift.
fn stage_slice_with_provenance() -> Project {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    // stage_slice_with_spec writes specs/login/spec.md by default;
    // the provenance gate gathers REQ ids across every spec.md, so we
    // can leave that path alone.
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("provenance.yaml"), CLEAN_RECONCILIATION_YAML)
        .expect("write provenance.yaml");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), CLEAN_EVIDENCE_YAML)
        .expect("write evidence");
    project
}

#[test]
fn validate_passes_on_clean_provenance_inputs() {
    let project = stage_slice_with_provenance();
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    // Adapter-level brief validation may still surface findings on the
    // synthetic slice — those route through different rule ids. Assert
    // that whatever surfaces on the report, *no* finding carries
    // `slice-provenance-drift` against clean inputs.
    assert_no_finding(assert.get_output(), "slice-provenance-drift");
}

#[test]
fn validate_req_id_drift() {
    let project = stage_slice_with_provenance();
    // Append a second REQ block to spec.md so spec.md has REQ-001 +
    // REQ-002 while provenance.yaml only knows REQ-001.
    let spec_path = project.slices_dir().join("my-slice/specs/login/spec.md");
    let extended = format!(
        "{CLEAN_SPEC_MD}\n\
         ### Requirement: Extra requirement\n\n\
         ID: REQ-002\n\
         Sources: [legacy-monolith]\n\
         Status: agreed\n\n\
         An undiscovered requirement.\n",
    );
    fs::write(&spec_path, extended).expect("rewrite spec.md");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let detail = find_finding_impact(assert.get_output(), "slice-provenance-drift");
    assert!(detail.contains("REQ-002"), "drift detail should name REQ-002, got: {detail}");
    assert!(
        detail.contains("missing from provenance.yaml"),
        "drift detail should mention the drift direction, got: {detail}"
    );
}

#[test]
fn validate_claim_drift_on_rename() {
    let project = stage_slice_with_provenance();
    // Rename the evidence claim id; provenance.yaml still cites the old one.
    let evidence_path = project.slices_dir().join("my-slice/evidence/legacy-monolith.yaml");
    let modified = CLEAN_EVIDENCE_YAML.replace("id: password-reset.request", "id: renamed-claim");
    fs::write(&evidence_path, modified).expect("rewrite evidence");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let detail = find_finding_impact(assert.get_output(), "slice-provenance-drift");
    assert!(
        detail.contains("legacy-monolith") && detail.contains("password-reset.request"),
        "drift detail should name the dangling (source, id) pair, got: {detail}"
    );
}

#[test]
fn validate_skips_drift_gate_without_provenance() {
    // Stage a slice with spec.md but no provenance.yaml — the drift gate
    // must be a silent no-op so older slices and pre-refine slices
    // still validate. (Any other adapter-level rules can still
    // surface, but no drift row may appear.)
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    assert_no_finding(assert.get_output(), "slice-provenance-drift");
}

#[test]
fn validate_no_drift_pre_synthesis() {
    // When provenance.yaml is present but spec.md is still pre-synthesis
    // (no Sources/Status lines), the drift gate must still gather
    // REQ ids from the bare `ID:` lines so a partially-refined slice
    // does not silently drift. This protects against the case where
    // the operator hand-deletes `Sources:` / `Status:` lines but
    // leaves the requirement intact.
    let spec = "### Requirement: Pre-synthesis body

ID: REQ-001

body without metadata lines yet
";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("provenance.yaml"), CLEAN_RECONCILIATION_YAML)
        .expect("write provenance");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), CLEAN_EVIDENCE_YAML)
        .expect("write evidence");
    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    assert_no_finding(assert.get_output(), "slice-provenance-drift");
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
// slice validate — `discovery-lead-synopsis-thin` advisory (RFC-29b-signal D2.1)
// ---------------------------------------------------------------------------

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
