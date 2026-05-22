//! Integration tests for the `specify slice` subcommand tree.
//!
//! Every test stands up a fresh `.specify/` project via `specify init`,
//! drives `specify slice *` through `assert_cmd`, and inspects both the
//! structured stdout (`--format json`) and the on-disk side effects the
//! verb is responsible for.
//!
//! Test style follows `tests/e2e.rs`: favour end-to-end execution of the
//! built binary over unit tests so the behaviour the skills consume is
//! the behaviour under test.

use std::fs;

mod common;
use common::{Project, parse_json, specify, stamp_slice_outcome};
use specify_domain::adapter::Operation;
use specify_domain::slice::OutcomeKind;

// ---------------------------------------------------------------------------
// slice create
// ---------------------------------------------------------------------------

#[test]
fn create_writes_dir_and_metadata() {
    let project = Project::init();
    let assert = specify()
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
    assert!(target.ends_with("/targets/omnia"));
    assert_eq!(value["created"], true);
    assert_eq!(value["restarted"], false);

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(slice_dir.is_dir(), "slice dir must exist");
    assert!(slice_dir.join("specs").is_dir(), "specs/ must exist");
    let meta = fs::read_to_string(slice_dir.join(".metadata.yaml")).expect("read metadata");
    assert!(meta.contains("status: refining"));
    assert!(meta.contains("target: file://"));
    assert!(meta.contains("created-at:"));
}

#[test]
fn create_rejects_uppercase_name() {
    let project = Project::init();
    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    for target in ["refined", "built"] {
        let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    // Refining -> Built is not a legal edge (must pass through refined).
    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "transition", "my-slice", "merged"])
        .assert()
        .code(2);
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "argument");
    assert_eq!(value["exit-code"], 2);
    let message = value["message"].as_str().expect("message string");
    assert!(
        message.contains("specify slice merge run"),
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let slice_dir = project.slices_dir().join("my-slice");

    // Adapter `alpha` — no baseline, should classify as `new`.
    fs::create_dir_all(slice_dir.join("specs/alpha")).unwrap();
    fs::write(slice_dir.join("specs/alpha/spec.md"), "# Alpha\n").unwrap();

    // Adapter `beta` — baseline exists, should classify as `modified`.
    fs::create_dir_all(project.specs_dir().join("beta")).unwrap();
    fs::write(project.specs_dir().join("beta/spec.md"), "# Beta baseline\n").unwrap();
    fs::create_dir_all(slice_dir.join("specs/beta")).unwrap();
    fs::write(slice_dir.join("specs/beta/spec.md"), "# Beta delta\n").unwrap();

    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "first"]).assert().success();
    specify().current_dir(project.root()).args(["slice", "create", "second"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "first", "--set", "login:new,oauth:new"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "second", "--set", "login:modified"])
        .assert()
        .success();

    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "alpha"]).assert().success();
    specify().current_dir(project.root()).args(["slice", "create", "beta"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "alpha", "--set", "aa:new"])
        .assert()
        .success();
    specify()
        .current_dir(project.root())
        .args(["slice", "touched-specs", "beta", "--set", "bb:new"])
        .assert()
        .success();

    let assert = specify()
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
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();

    let assert = specify()
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

// ---------------------------------------------------------------------------
// slice status
// ---------------------------------------------------------------------------

#[test]
fn status_by_name_returns_single_entry() {
    let project = Project::init().with_schemas();
    specify()
        .current_dir(project.root())
        .args(["slice", "create", "only-slice"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "status", "only-slice"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let entry = &value["slice"];
    assert_eq!(entry["name"], "only-slice");
    assert_eq!(entry["status"], "refining");
}

// ---------------------------------------------------------------------------
// slice outcome show
// ---------------------------------------------------------------------------

/// Naive RFC3339 sanity check sufficient for integration tests: `YYYY-MM-DDT...`.
fn looks_like_rfc3339(s: &str) -> bool {
    s.len() >= 20
        && s.chars().nth(4) == Some('-')
        && s.chars().nth(7) == Some('-')
        && s.chars().nth(10) == Some('T')
}

#[test]
fn metadata_without_outcome_still_parses() {
    use specify_domain::slice::SliceMetadata;
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
fn outcome_returns_stamped_as_json() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();
    stamp_slice_outcome(
        &project,
        "foo",
        Operation::Build,
        OutcomeKind::Success,
        "5/5 tasks",
        Some("trailing newline"),
    );

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["name"], "foo");
    let outcome = &value["outcome"];
    assert_eq!(outcome["phase"].as_str(), Some("build"));
    assert_eq!(outcome["outcome"].as_str(), Some("success"));
    assert_eq!(outcome["summary"].as_str(), Some("5/5 tasks"));
    assert_eq!(outcome["context"].as_str(), Some("trailing newline"));
    let at = outcome["at"].as_str().expect("at is a string");
    assert!(looks_like_rfc3339(at), "at should be RFC3339, got {at}");
}

#[test]
fn outcome_emits_null_when_unstamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["name"], "foo");
    assert!(
        value["outcome"].is_null(),
        "outcome must be null when not yet stamped, got: {}",
        value["outcome"]
    );
    assert_eq!(assert.get_output().status.code(), Some(0));
}

#[test]
fn outcome_null_context_when_unstamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();
    stamp_slice_outcome(&project, "foo", Operation::Shape, OutcomeKind::Success, "ok", None);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "foo"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    let outcome = &value["outcome"];
    assert!(
        outcome["context"].is_null(),
        "context must render as null when absent, got: {}",
        outcome["context"]
    );
}

#[test]
fn outcome_text_output_stamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();
    stamp_slice_outcome(&project, "foo", Operation::Build, OutcomeKind::Success, "5/5 tasks", None);

    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "show", "foo"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    assert_eq!(stdout.trim_end(), "foo: build/success — 5/5 tasks");
}

#[test]
fn outcome_text_output_unstamped() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "foo"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "outcome", "show", "foo"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    assert_eq!(stdout.trim_end(), "foo: no outcome stamped");
}

#[test]
fn outcome_errors_on_missing_slice() {
    let project = Project::init();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "ghost"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    let msg = value["message"].as_str().unwrap_or("");
    assert!(msg.contains("not found"), "expected 'not found' in message, got: {msg}");
}

#[test]
fn outcome_falls_back_to_archive() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "bar"]).assert().success();
    stamp_slice_outcome(
        &project,
        "bar",
        Operation::Merge,
        OutcomeKind::Success,
        "Merged 2 spec(s) into baseline",
        None,
    );

    // Simulate the archive move that `specify merge` performs.
    let slices_dir = project.root().join(".specify/slices");
    let archive_dir = project.root().join(".specify/archive");
    fs::create_dir_all(&archive_dir).unwrap();
    fs::rename(slices_dir.join("bar"), archive_dir.join("2026-04-24-bar")).unwrap();

    // The active slice directory is gone; outcome should resolve from archive.
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "bar"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["name"], "bar");
    let outcome = &value["outcome"];
    assert_eq!(outcome["phase"].as_str(), Some("merge"));
    assert_eq!(outcome["outcome"].as_str(), Some("success"));
    assert_eq!(outcome["summary"].as_str(), Some("Merged 2 spec(s) into baseline"));
}

#[test]
fn outcome_archive_picks_most_recent() {
    let project = Project::init();

    // Create and stamp two archived versions with different created-at timestamps.
    let archive_dir = project.root().join(".specify/archive");
    fs::create_dir_all(&archive_dir).unwrap();

    for (date, summary) in [("2026-01-01-baz", "old run"), ("2026-04-24-baz", "latest run")] {
        let dir = archive_dir.join(date);
        fs::create_dir_all(&dir).unwrap();
        let created_at = if date.starts_with("2026-01") {
            "2026-01-01T00:00:00Z"
        } else {
            "2026-04-24T00:00:00Z"
        };
        let yaml = format!(
            "target: omnia\nstatus: merged\ncreated-at: \"{created_at}\"\noutcome:\n  phase: merge\n  outcome: success\n  at: \"{created_at}\"\n  summary: \"{summary}\"\n"
        );
        fs::write(dir.join(".metadata.yaml"), yaml).unwrap();
    }

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "outcome", "show", "baz"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(
        value["outcome"]["summary"].as_str(),
        Some("latest run"),
        "should pick the most recent archive entry"
    );
}

#[test]
fn phase_outcome_round_trips_serde() {
    use specify_domain::slice::Outcome;
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

// ---- RFC-25 top-level help surfaces source/target axis verbs ----

#[test]
fn top_level_help_lists_source_and_target_axis_verbs() {
    let assert = specify().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("slice"), "RFC-25 --help must still list `slice`, got:\n{stdout}");
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("source ")),
        "RFC-25 --help must list the `source` axis verb, got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("target ")),
        "RFC-25 --help must list the `target` axis verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("change ")),
        "RFC-25 --help must NOT list the retired `change` verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("adapter ")),
        "RFC-25 --help must NOT list the retired `adapter` verb, got:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// RFC-25 §Requirement block contract — `slice validate` provenance gate
// ---------------------------------------------------------------------------

/// Stage a slice on disk and seed `<slice>/specs/login/spec.md`
/// directly, plus optionally a `plan.yaml` at the project root, so the
/// provenance gate inside `specify slice validate` has both the spec
/// file and a plan-level source-bindings context to cross-validate
/// against. Returns the project handle so the caller can drive
/// `specify slice validate` on it.
fn stage_slice_with_spec(spec_md: &str, plan_yaml: Option<&str>) -> Project {
    let project = Project::init().with_schemas();
    specify().current_dir(project.root()).args(["slice", "create", "my-slice"]).assert().success();
    let specs_dir = project.slices_dir().join("my-slice/specs/login");
    fs::create_dir_all(&specs_dir).expect("mkdir specs/login");
    fs::write(specs_dir.join("spec.md"), spec_md).expect("write spec.md");
    if let Some(yaml) = plan_yaml {
        project.seed_plan(yaml);
    }
    project
}

/// Validate-fail goldens carry a `validation` discriminant; assert
/// that the wire envelope holds the expected `rule_id` exactly once.
fn assert_provenance_fail_rule(stderr: &[u8], rule_id: &str) {
    let value = parse_json(stderr);
    assert_eq!(value["error"], "validation", "wire envelope must be `validation`");
    assert_eq!(value["exit-code"], 2);
    let results = value["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["rule-id"] == rule_id),
        "expected rule_id `{rule_id}` in results: {results:#?}"
    );
}

const PLAN_WITH_LEGACY_MONOLITH: &str = "\
name: rfc25-prov
lifecycle: pending
sources:
  legacy-monolith: ./legacy
slices:
  - name: my-slice
    status: pending
    sources:
      - { key: legacy-monolith, candidate: my-slice }
";

#[test]
fn validate_rejects_missing_id_with_exit_two() {
    let spec = "### Requirement: Missing id\n\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n\n\
                body\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-id-missing");
}

#[test]
fn validate_rejects_malformed_id_with_exit_two() {
    let spec = "### Requirement: Malformed id\n\n\
                ID: REQ-1\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-id-malformed");
}

#[test]
fn validate_rejects_missing_sources_with_exit_two() {
    let spec = "### Requirement: No sources\n\n\
                ID: REQ-001\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-sources-missing");
}

#[test]
fn validate_rejects_missing_status_with_exit_two() {
    let spec = "### Requirement: No status\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(&assert.get_output().stderr, "spec.requirement-status-missing");
}

#[test]
fn validate_rejects_unknown_status_value_with_exit_two() {
    let spec = "### Requirement: Bogus status\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n\
                Status: maybe\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(
        &assert.get_output().stderr,
        "spec.requirement-status-unknown-value",
    );
}

#[test]
fn validate_rejects_source_key_not_in_plan_with_exit_two() {
    let spec = "### Requirement: Stray source key\n\n\
                ID: REQ-001\n\
                Sources: [phantom]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(
        &assert.get_output().stderr,
        "spec.requirement-source-key-undefined",
    );
}

#[test]
fn validate_rejects_tag_status_mismatch_with_exit_two() {
    let spec = "### Requirement: Lying tag [divergence]\n\n\
                ID: REQ-001\n\
                Sources: [legacy-monolith]\n\
                Status: agreed\n";
    let project = stage_slice_with_spec(spec, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    assert_provenance_fail_rule(
        &assert.get_output().stderr,
        "spec.requirement-tag-status-mismatch",
    );
}

#[test]
fn validate_skips_provenance_when_no_metadata_lines_present() {
    // Pre-RFC-25 (or pre-synthesis) state. The provenance gate must
    // not fire and the slice progresses to the existing adapter rule
    // run. The adapter rules will still surface deferred /
    // pass-style results — we only assert the provenance rule ids
    // are NOT present.
    let spec = "### Requirement: Pre-RFC-25 body\n\n\
                ID: REQ-001\n\n\
                body that has no Sources or Status yet\n";
    let project = stage_slice_with_spec(spec, None);
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let stderr = assert.get_output().stderr.clone();
    // Whether the run passes or fails (existing adapter rules may
    // still produce findings on the synthetic slice), no provenance
    // rule should appear.
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stderr)
        && let Some(results) = value["results"].as_array()
    {
        for r in results {
            let rule_id = r["rule-id"].as_str().unwrap_or("");
            assert!(
                !rule_id.starts_with("spec.requirement-"),
                "no provenance rule should fire on a pre-RFC-25 spec.md, got: {rule_id}"
            );
        }
    }
}
