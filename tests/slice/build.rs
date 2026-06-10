//! Integration tests for `specify slice build` (RFC-29d M3 / D6).
//!
//! Covers the two-phase agent contract that mirrors `specify source
//! survey` / `extract`: `prepare` assembles + persists a schema-valid
//! build request and emits `target.execution.agent` without
//! transitioning the slice; `finalize` frames entry with
//! `slice.build.started`, validates the agent-produced report, gates the
//! `built` transition, and emits `slice.build.succeeded` /
//! `slice.build.failed`. Also covers the `success`-with-blocking-finding
//! rejection and the `execution: tool` unsupported seam.

use std::fs;

use serde_json::Value;

use crate::common::{Project, parse_json, read_journal_normalized, specify_cmd};

/// Create `my-slice`, seed a `specs/<unit>/spec.md` so the assembled
/// request carries a non-empty `specs[]`, and transition it to
/// `refined` — the lifecycle state `slice build` gates out of.
fn stage_refined_slice(project: &Project) {
    specify_cmd()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    let spec_dir = project.slices_dir().join("my-slice/specs/identity");
    fs::create_dir_all(&spec_dir).expect("mkdir specs/identity");
    fs::write(spec_dir.join("spec.md"), "# Identity spec\n").expect("write spec.md");
    specify_cmd()
        .current_dir(project.root())
        .args(["slice", "transition", "my-slice", "refined"])
        .assert()
        .success();
}

/// Write `report` to `.specify/slices/my-slice/build/report.yaml`,
/// standing in for the agent's `build` brief output.
fn write_report(project: &Project, report: &str) {
    let build_dir = project.slices_dir().join("my-slice/build");
    fs::create_dir_all(&build_dir).expect("mkdir build dir");
    fs::write(build_dir.join("report.yaml"), report).expect("write report.yaml");
}

/// Collect the `event` ids in the slice's journal, in append order.
fn event_ids(events: &[Value]) -> Vec<&str> {
    events.iter().filter_map(|e| e["event"].as_str()).collect()
}

fn metadata(project: &Project) -> String {
    fs::read_to_string(project.slices_dir().join("my-slice/metadata.yaml")).expect("read metadata")
}

const SUCCESS_REPORT: &str = "\
version: 1
slice: my-slice
target: omnia@v1
status: success
findings: []
";

/// A `status: success` report carrying a blocking (`critical`,
/// default-`open` `violation`) finding — the CLI rejects this with
/// `target-build-success-with-blocking-finding`.
const SUCCESS_WITH_BLOCKING_REPORT: &str = "\
version: 1
slice: my-slice
target: omnia@v1
status: success
findings:
  - id: DIAG-0001
    title: Generated code fails to compile
    severity: critical
    source: tool
    artifact: code
    evidence:
      kind: snippet
      value: \"error[E0382]: borrow of moved value\"
    impact: The generated crate does not compile, so the slice cannot merge.
    remediation: Fix the borrow error before reporting success.
    fingerprint: \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"
";

// ---------------------------------------------------------------------------
// agent prepare
// ---------------------------------------------------------------------------

#[test]
fn prepare_writes_request_no_transition() {
    let project = Project::init();
    stage_refined_slice(&project);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "build", "my-slice"])
        .assert()
        .success();

    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["slice"], "my-slice");
    assert_eq!(body["target"], "omnia");
    assert_eq!(body["execution"], "agent");
    let request_field = body["request"].as_str().expect("request path string");
    assert!(
        request_field.ends_with(".specify/slices/my-slice/build/request.yaml"),
        "handoff request path: {request_field}"
    );
    let report_field = body["report"].as_str().expect("report path string");
    assert!(
        report_field.ends_with(".specify/slices/my-slice/build/report.yaml"),
        "handoff report path: {report_field}"
    );
    assert!(
        body["build-brief"].as_str().expect("build-brief string").ends_with("briefs/build.md"),
        "handoff must point at the build brief"
    );

    // prepare wrote a schema-valid request (it schema-validates before
    // the write, so a successful prepare proves validity); spot-check
    // the closed-shape fields.
    let request_path = project.slices_dir().join("my-slice/build/request.yaml");
    assert!(request_path.is_file(), "prepare must write build/request.yaml");
    let raw = fs::read_to_string(&request_path).expect("read request.yaml");
    assert!(raw.contains("version: 1"), "request carries version, got:\n{raw}");
    assert!(raw.contains("slice: my-slice"), "request carries slice, got:\n{raw}");
    assert!(raw.contains("project-dir:"), "request carries project-dir, got:\n{raw}");
    assert!(raw.contains("specs/identity/spec.md"), "request enumerates the spec, got:\n{raw}");

    // prepare emits the agent-dispatch signal, but NOT the
    // `slice.build.started` frame — that is owned by finalize so a
    // prepare-time abort never leaves a dangling `started`.
    let events = read_journal_normalized(project.root());
    let agent = events
        .iter()
        .find(|e| e["event"] == "target.execution.agent")
        .expect("prepare emits target.execution.agent");
    assert_eq!(agent["payload"]["slice"], "my-slice", "agent event names the slice: {agent}");
    assert_eq!(agent["payload"]["target"], "omnia", "agent event names the target: {agent}");
    assert!(
        !event_ids(&events).contains(&"slice.build.started"),
        "prepare must NOT emit slice.build.started (finalize owns it), got: {:?}",
        event_ids(&events)
    );

    // prepare must not transition the slice.
    assert!(
        metadata(&project).contains("status: refined"),
        "prepare must leave the slice at refined; got:\n{}",
        metadata(&project)
    );
}

// ---------------------------------------------------------------------------
// agent finalize
// ---------------------------------------------------------------------------

#[test]
fn finalize_validates_and_gates_built() {
    let project = Project::init();
    stage_refined_slice(&project);
    write_report(&project, SUCCESS_REPORT);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "build", "my-slice", "--phase", "finalize"])
        .assert()
        .success();

    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["slice"], "my-slice");
    assert_eq!(body["target"], "omnia@v1");
    assert_eq!(body["status"], "success");
    assert_eq!(body["findings"], 0);

    let events = read_journal_normalized(project.root());
    let ids = event_ids(&events);
    assert!(
        ids.contains(&"slice.build.started"),
        "finalize frames entry with slice.build.started, got: {ids:?}"
    );
    assert!(
        ids.contains(&"slice.build.succeeded"),
        "finalize emits slice.build.succeeded, got: {ids:?}"
    );

    // The gate transitioned the slice to `built`.
    assert!(
        metadata(&project).contains("status: built"),
        "finalize gates the built transition; got:\n{}",
        metadata(&project)
    );
}

#[test]
fn finalize_rejects_success_blocking() {
    let project = Project::init();
    stage_refined_slice(&project);
    write_report(&project, SUCCESS_WITH_BLOCKING_REPORT);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "build", "my-slice", "--phase", "finalize"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = parse_json(&assert.get_output().stderr);
    assert_eq!(stderr["error"], "target-build-success-with-blocking-finding");

    // The rejection neither transitions the slice nor records success.
    assert!(
        metadata(&project).contains("status: refined"),
        "a rejected report must not transition; got:\n{}",
        metadata(&project)
    );
    let events = read_journal_normalized(project.root());
    let ids = event_ids(&events);
    assert!(
        ids.contains(&"slice.build.failed"),
        "a rejected report emits slice.build.failed, got: {ids:?}"
    );
    assert!(
        !ids.contains(&"slice.build.succeeded"),
        "a rejected report must not emit slice.build.succeeded, got: {ids:?}"
    );
}

#[test]
fn finalize_missing_report_errors() {
    let project = Project::init();
    stage_refined_slice(&project);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "build", "my-slice", "--phase", "finalize"])
        .assert()
        .failure();
    let stderr = parse_json(&assert.get_output().stderr);
    assert_eq!(stderr["error"], "target-build-report-missing");
    assert!(
        metadata(&project).contains("status: refined"),
        "a missing report must not transition the slice"
    );
}

// ---------------------------------------------------------------------------
// execution: tool seam
// ---------------------------------------------------------------------------

#[test]
fn tool_execution_reports_unsupported_seam() {
    let project = Project::init();
    stage_refined_slice(&project);

    // `init` caches the resolved manifest; flip it to `execution: tool`
    // so the verb takes the tool branch. No build tool dispatch is
    // wired, so the dispatch is a clear unsupported seam.
    let cached = project.root().join(".specify/cache/manifests/targets/omnia/adapter.yaml");
    let raw = fs::read_to_string(&cached).expect("read cached adapter.yaml");
    fs::write(&cached, raw.replace("execution: agent", "execution: tool"))
        .expect("rewrite adapter execution mode");

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "build", "my-slice"])
        .assert()
        .failure();
    let stderr = parse_json(&assert.get_output().stderr);
    assert_eq!(stderr["error"], "target-build-tool-unsupported");
}
