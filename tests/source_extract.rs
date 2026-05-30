//! Integration tests for `specrun source extract` (RFC-29a §`extract`).
//!
//! Covers source-key resolution against `plan.yaml.sources`, the agent
//! two-phase dispatch (prepare prints the extract handoff envelope —
//! with `evidence-dir`, a single lead, and either `source-dir` or
//! `value-inline` — and emits `source.execution.agent`; finalize
//! validates-before-visible, persists the Evidence, and emits
//! `slice.extract.cache-miss` under the forced opt-out), the
//! validate-before-visible guarantee that an invalid Evidence document
//! persists no file, and the value-bound `intent` path.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

mod common;
use common::{Project, parse_stderr, parse_stdout, repo_root, specrun};

/// Stage the path-bound `code-typescript` source adapter (the in-repo
/// fixture ships only `adapter.yaml`; author the `extract` brief the
/// fingerprint hashes).
fn stage_code_typescript(project: &Project) {
    let src = repo_root().join(
        "crates/workflow/tests/fixtures/plugins/adapters/sources/code-typescript/adapter.yaml",
    );
    let adapter_dir = project.root().join("adapters/sources/code-typescript");
    fs::create_dir_all(adapter_dir.join("briefs")).expect("create adapter briefs dir");
    fs::copy(&src, adapter_dir.join("adapter.yaml")).expect("copy adapter.yaml");
    fs::write(adapter_dir.join("briefs/extract.md"), "# extract brief\n")
        .expect("write extract brief");
}

/// Author a value-bound `intent` source adapter (`execution: agent`).
fn stage_intent(project: &Project) {
    let adapter_dir = project.root().join("adapters/sources/intent");
    fs::create_dir_all(adapter_dir.join("briefs")).expect("create adapter briefs dir");
    fs::write(
        adapter_dir.join("adapter.yaml"),
        "name: intent
version: 1
axis: source
execution: agent
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
description: Operator-supplied free-form intent.
",
    )
    .expect("write adapter.yaml");
    fs::write(adapter_dir.join("briefs/extract.md"), "# extract brief\n")
        .expect("write extract brief");
}

fn seed_plan_with_legacy_source(project: &Project) {
    project.seed_plan(
        "name: platform-v2
sources:
  legacy:
    adapter: code-typescript
    path: vendor/legacy
slices:
  - name: identity
    project: default
    status: pending
",
    );
}

fn seed_plan_with_value_source(project: &Project) {
    project.seed_plan(
        "name: platform-v2
sources:
  brief:
    adapter: intent
    value: Build a password reset flow.
slices:
  - name: identity
    project: default
    status: pending
",
    );
}

fn extract_scratch_dir(project: &Project, adapter: &str, slice: &str) -> PathBuf {
    project.root().join(format!(".specify/.cache/extractions/{adapter}/{slice}/scratch"))
}

fn slice_evidence_path(project: &Project, slice: &str, source_key: &str) -> PathBuf {
    project.root().join(format!(".specify/slices/{slice}/evidence/{source_key}.yaml"))
}

fn journal_events(project: &Project) -> Vec<Value> {
    let path = project.root().join(".specify/journal.jsonl");
    let raw = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read journal.jsonl: {err}"));
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("journal line is JSON"))
        .collect()
}

const VALID_EVIDENCE: &str = "\
source: legacy
adapter: code-typescript
authority: behaviour
lead: user-registration
claims: []
";

#[test]
fn agent_prepare_prints_envelope_and_emits_execution_event() {
    let project = Project::init();
    stage_code_typescript(&project);
    seed_plan_with_legacy_source(&project);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "extract", "legacy", "user-registration"])
        .args(["--slice", "identity"])
        .assert()
        .success();

    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "code-typescript");
    assert_eq!(body["version"], 1);
    assert_eq!(body["execution"], "agent");

    // extract — unlike survey — carries evidence-dir.
    let evidence = body["evidence-dir"].as_str().expect("evidence-dir str");
    assert!(
        evidence.ends_with(".specify/slices/identity/evidence"),
        "evidence-dir must target the slice evidence tree: {evidence}"
    );
    let scratch = body["scratch-dir"].as_str().expect("scratch-dir str");
    assert!(
        scratch.ends_with(".specify/.cache/extractions/code-typescript/identity/scratch"),
        "scratch-dir {scratch} must key under the slice segment"
    );
    let source_dir = body["source-dir"].as_str().expect("path binding carries source-dir");
    assert!(source_dir.ends_with("vendor/legacy"), "source-dir: {source_dir}");
    assert!(
        body.get("value-inline").is_none(),
        "a path binding must not carry value-inline, got:\n{body}"
    );
    let leads: Vec<&str> =
        body["leads"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(leads, vec!["user-registration"], "extract carries the single lead-id");

    // prepare builds scratch up front and scaffolds the evidence target.
    assert!(
        extract_scratch_dir(&project, "code-typescript", "identity").is_dir(),
        "prepare must create the scratch dir"
    );
    assert!(
        project.root().join(".specify/slices/identity/evidence").is_dir(),
        "prepare must scaffold the slice evidence/ dir"
    );

    let events = journal_events(&project);
    assert_eq!(events.len(), 1, "prepare emits exactly one event");
    assert_eq!(events[0]["event"], "source.execution.agent");
    assert_eq!(events[0]["payload"]["source-key"], "legacy");
    assert_eq!(events[0]["payload"]["adapter"], "code-typescript");
    assert_eq!(events[0]["payload"]["operation"], "extract");
}

#[test]
fn agent_prepare_value_bound_source_carries_value_inline() {
    let project = Project::init();
    stage_intent(&project);
    seed_plan_with_value_source(&project);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "extract", "brief", "password-reset"])
        .args(["--slice", "identity"])
        .assert()
        .success();

    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "intent");
    assert!(
        body.get("source-dir").is_none(),
        "value-bound source must NOT carry source-dir, got:\n{body}"
    );
    assert_eq!(
        body["value-inline"], "Build a password reset flow.",
        "value-bound source carries the literal binding body"
    );
    assert!(body.get("evidence-dir").is_some(), "extract always carries evidence-dir");
    let leads: Vec<&str> =
        body["leads"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(leads, vec!["password-reset"]);
}

#[test]
fn agent_finalize_persists_evidence_and_emits_cache_miss() {
    let project = Project::init();
    stage_code_typescript(&project);
    seed_plan_with_legacy_source(&project);
    // The fingerprint canonicalises the bound source path, so it must exist.
    fs::create_dir_all(project.root().join("vendor/legacy")).expect("create bound source dir");

    // Stand in for the agent: write the produced Evidence into scratch.
    let scratch = extract_scratch_dir(&project, "code-typescript", "identity");
    fs::create_dir_all(&scratch).expect("create scratch dir");
    fs::write(scratch.join("evidence.yaml"), VALID_EVIDENCE).expect("write evidence.yaml");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "extract", "legacy", "user-registration"])
        .args(["--slice", "identity", "--phase", "finalize"])
        .assert()
        .success();

    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "code-typescript");
    assert_eq!(body["source-key"], "legacy");
    assert_eq!(body["slice"], "identity");
    assert_eq!(body["lead"], "user-registration");
    assert_eq!(body["cache"], "miss", "agent execution forces a cache miss");
    assert_eq!(body["reason"], "adapter-opt-out");
    let fingerprint = body["fingerprint"].as_str().expect("fingerprint str");
    assert!(fingerprint.starts_with("sha256:"), "fingerprint: {fingerprint}");

    // The validated Evidence is now persisted to the slice evidence path.
    let persisted = slice_evidence_path(&project, "identity", "legacy");
    assert!(persisted.is_file(), "Evidence persisted to {}", persisted.display());
    assert_eq!(fs::read_to_string(&persisted).expect("read persisted"), VALID_EVIDENCE);

    let events = journal_events(&project);
    let miss = events
        .iter()
        .find(|e| e["event"] == "slice.extract.cache-miss")
        .expect("a slice.extract.cache-miss event");
    assert_eq!(miss["payload"]["slice-name"], "identity");
    assert_eq!(miss["payload"]["source-key"], "legacy");
    assert_eq!(miss["payload"]["adapter"], "code-typescript");
    assert_eq!(miss["payload"]["reason"], "adapter-opt-out");
    assert_eq!(miss["payload"]["fingerprint"], fingerprint);
}

#[test]
fn agent_finalize_value_bound_source_persists_evidence() {
    let project = Project::init();
    stage_intent(&project);
    seed_plan_with_value_source(&project);

    let scratch = extract_scratch_dir(&project, "intent", "identity");
    fs::create_dir_all(&scratch).expect("create scratch dir");
    fs::write(
        scratch.join("evidence.yaml"),
        "source: brief\nadapter: intent\nauthority: intent\nlead: password-reset\nclaims: []\n",
    )
    .expect("write evidence.yaml");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "extract", "brief", "password-reset"])
        .args(["--slice", "identity", "--phase", "finalize"])
        .assert()
        .success();

    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "intent");
    assert_eq!(body["cache"], "miss");
    assert_eq!(body["reason"], "adapter-opt-out");

    assert!(
        slice_evidence_path(&project, "identity", "brief").is_file(),
        "value-bound Evidence persists with no $SOURCE_DIR present"
    );
}

#[test]
fn agent_finalize_invalid_evidence_persists_no_file() {
    let project = Project::init();
    stage_code_typescript(&project);
    seed_plan_with_legacy_source(&project);
    fs::create_dir_all(project.root().join("vendor/legacy")).expect("create bound source dir");

    // Missing the required `claims` field — parses as YAML but fails the schema.
    let scratch = extract_scratch_dir(&project, "code-typescript", "identity");
    fs::create_dir_all(&scratch).expect("create scratch dir");
    fs::write(
        scratch.join("evidence.yaml"),
        "source: legacy\nadapter: code-typescript\nauthority: behaviour\nlead: user-registration\n",
    )
    .expect("write invalid evidence.yaml");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "extract", "legacy", "user-registration"])
        .args(["--slice", "identity", "--phase", "finalize"])
        .assert()
        .failure();

    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "evidence-schema");
    assert_eq!(stderr["exit-code"], 2);

    // Validate-before-visible: no Evidence file lands on the slice path.
    assert!(
        !slice_evidence_path(&project, "identity", "legacy").exists(),
        "an invalid Evidence document must persist no file"
    );
    // No cache event fires for an invalid Evidence document.
    assert!(
        !project.root().join(".specify/journal.jsonl").exists()
            || !journal_events(&project).iter().any(|e| {
                e["event"] == "slice.extract.cache-miss" || e["event"] == "slice.extract.cache-hit"
            }),
        "invalid Evidence must not emit a cache event"
    );
}

#[test]
fn unknown_source_key_errors() {
    let project = Project::init();
    stage_code_typescript(&project);
    seed_plan_with_legacy_source(&project);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "extract", "not-a-source", "user-registration"])
        .args(["--slice", "identity"])
        .assert()
        .failure();

    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "source-key-unknown");
    assert_eq!(stderr["exit-code"], 1);
}
