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

// ---------------------------------------------------------------------------
// RFC-27 §D4 — `slice validate` fusion drift gate + `slice fusion show`
// ---------------------------------------------------------------------------

/// Minimal fusion.yaml for a slice named `my-slice` with one
/// requirement `REQ-001` whose single contributing claim cites
/// `legacy-monolith :: REQ-001` (the same id we'll seed the evidence
/// file with by default).
const CLEAN_FUSION_YAML: &str = "version: 1
slice: my-slice
generated-at: 2026-05-22T13:15:00Z
generator: specify@2.1.0
requirements:
  - id: REQ-001
    status: agreed
    sources: [legacy-monolith]
    contributing-claims:
      - source: legacy-monolith
        claim-id: REQ-001
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

const CLEAN_EVIDENCE_YAML: &str = "source: legacy-monolith
adapter: code-typescript
authority: behaviour
candidate: my-slice
claims:
  - kind: requirement
    claim-id: REQ-001
    statement: \"Password reset request returns a 200 response.\"
    path: src/users/reset.ts#L42
";

/// Stage a fully-wired slice with fusion.yaml + spec.md + evidence
/// so the drift gate has every input it needs and the baseline test
/// fixture validates clean. Caller may then mutate any file before
/// re-running `slice validate` to exercise drift.
fn stage_slice_with_fusion() -> Project {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    // stage_slice_with_spec writes specs/login/spec.md by default;
    // the fusion gate gathers REQ ids across every spec.md, so we
    // can leave that path alone.
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("fusion.yaml"), CLEAN_FUSION_YAML).expect("write fusion.yaml");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), CLEAN_EVIDENCE_YAML)
        .expect("write evidence");
    project
}

#[test]
fn validate_passes_on_clean_fusion_inputs() {
    let project = stage_slice_with_fusion();
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let stderr = assert.get_output().stderr.clone();
    let code = assert.get_output().status.code();
    if code != Some(0) {
        // Adapter-level brief validation may still surface findings on
        // the synthetic slice — those would route through different
        // rule ids. Assert that whatever surfaces, *no* row carries
        // `slice-fusion-drift` against clean inputs.
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stderr)
            && let Some(results) = value["results"].as_array()
        {
            for r in results {
                let rule_id = r["rule-id"].as_str().unwrap_or("");
                assert_ne!(
                    rule_id, "slice-fusion-drift",
                    "no drift row may appear on clean inputs; got results: {results:#?}"
                );
            }
        }
    }
}

#[test]
fn validate_detects_req_id_drift_when_spec_md_has_extra_requirement() {
    let project = stage_slice_with_fusion();
    // Append a second REQ block to spec.md so spec.md has REQ-001 +
    // REQ-002 while fusion.yaml only knows REQ-001.
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

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "validation");
    let results = value["results"].as_array().expect("results array");
    let detail = results
        .iter()
        .find(|r| r["rule-id"] == "slice-fusion-drift")
        .and_then(|r| r["detail"].as_str())
        .expect("slice-fusion-drift row must be present");
    assert!(detail.contains("REQ-002"), "drift detail should name REQ-002, got: {detail}");
    assert!(
        detail.contains("missing from fusion.yaml"),
        "drift detail should mention the drift direction, got: {detail}"
    );
}

#[test]
fn validate_detects_contributing_claim_drift_when_evidence_claim_renamed() {
    let project = stage_slice_with_fusion();
    // Rename the evidence claim id; fusion.yaml still cites the old one.
    let evidence_path = project.slices_dir().join("my-slice/evidence/legacy-monolith.yaml");
    let modified = CLEAN_EVIDENCE_YAML.replace("claim-id: REQ-001", "claim-id: REQ-999-renamed");
    fs::write(&evidence_path, modified).expect("rewrite evidence");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "validation");
    let results = value["results"].as_array().expect("results array");
    let detail = results
        .iter()
        .find(|r| r["rule-id"] == "slice-fusion-drift")
        .and_then(|r| r["detail"].as_str())
        .expect("slice-fusion-drift row must be present");
    assert!(
        detail.contains("legacy-monolith") && detail.contains("REQ-001"),
        "drift detail should name the dangling (source, claim-id) pair, got: {detail}"
    );
}

#[test]
fn validate_skips_drift_gate_when_fusion_yaml_absent() {
    // Stage a slice with spec.md but no fusion.yaml — the drift gate
    // must be a silent no-op so older slices and pre-refine slices
    // still validate. (Any other adapter-level rules can still
    // surface, but no drift row may appear.)
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
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
                rule_id, "slice-fusion-drift",
                "drift gate must skip when fusion.yaml is absent"
            );
        }
    }
}

#[test]
fn fusion_show_json_round_trips_byte_stable() {
    let project = stage_slice_with_fusion();
    // Two consecutive invocations must produce byte-identical
    // stdout — the JSON path parses fusion.yaml into FusionIndex and
    // re-serialises through serde_json, so timestamp / map-key
    // ordering must be deterministic.
    let first = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "fusion", "show", "my-slice"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "fusion", "show", "my-slice"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(first, second, "fusion show --format json must be byte-stable");

    // Spot-check the parsed shape so we know we are emitting the
    // FusionIndex serde form, not the raw YAML bytes.
    let value: serde_json::Value = serde_json::from_slice(&first).expect("valid JSON");
    assert_eq!(value["slice"], "my-slice");
    assert_eq!(value["version"], 1);
    assert_eq!(value["generator"], "specify@2.1.0");
    let requirements = value["requirements"].as_array().expect("requirements array");
    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[0]["id"], "REQ-001");
    assert_eq!(requirements[0]["resolution"], "single-source");
    let claim = &requirements[0]["contributing-claims"][0];
    assert_eq!(claim["source"], "legacy-monolith");
    assert_eq!(claim["claim-id"], "REQ-001");
    assert_eq!(claim["value"], "Password reset request returns a 200 response.");
}

#[test]
fn fusion_show_text_prints_inline_value_payloads() {
    let project = stage_slice_with_fusion();
    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "fusion", "show", "my-slice"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("slice: my-slice"), "expected slice header, got:\n{stdout}");
    assert!(stdout.contains("REQ-001"), "expected REQ-001 requirement section, got:\n{stdout}");
    assert!(
        stdout.contains("resolution: single-source"),
        "expected resolution line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("legacy-monolith :: REQ-001"),
        "expected contributing-claim line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("value: Password reset request returns a 200 response."),
        "expected inline value payload, got:\n{stdout}"
    );
    assert!(
        stdout.contains("path:  src/users/reset.ts#L42"),
        "expected path anchor, got:\n{stdout}"
    );
}

#[test]
fn fusion_show_reports_missing_fusion_file_with_diag_exit_one() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "fusion", "show", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-fusion-not-found");
}

#[test]
fn fusion_show_rejects_schema_invalid_file_with_exit_two() {
    let project = stage_slice_with_spec(CLEAN_SPEC_MD, Some(PLAN_WITH_LEGACY_MONOLITH));
    // `version: 0` parses cleanly as u32 but the schema demands >= 1
    // so the failure routes through Error::Validation (exit 2)
    // rather than YAML deserialise (exit 1).
    let fusion_path = project.slices_dir().join("my-slice/fusion.yaml");
    fs::write(
        &fusion_path,
        "version: 0
slice: my-slice
generated-at: 2026-05-22T13:15:00Z
generator: specify@2.1.0
requirements: []
",
    )
    .expect("write fusion");
    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "fusion", "show", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "validation");
    let results = value["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["rule-id"] == "fusion-schema"),
        "expected fusion-schema row, got: {results:#?}"
    );
}

#[test]
fn validate_skipped_drift_gate_does_not_fire_on_pre_synthesis_spec() {
    // When fusion.yaml is present but spec.md is still pre-synthesis
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
    fs::write(slice_dir.join("fusion.yaml"), CLEAN_FUSION_YAML).expect("write fusion");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), CLEAN_EVIDENCE_YAML)
        .expect("write evidence");
    let assert = specify()
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
                rule_id, "slice-fusion-drift",
                "drift gate must accept matching REQ ids even when Sources/Status metadata is absent"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RFC-27 §Acceptance scenario #26-2 — per-slice `authority-override` on a
// `criterion` claim. Behaviour-class production fixtures (`runtime`) flip the
// default ordering so docs lose; `fusion.yaml.requirements[].resolution` is
// `per-slice-override` and `resolution-trace.step` reads
// `per-slice-authority-override`.
//
// The CLI does not own the synthesis resolver in v2.1 (it is skill-driven per
// RFC-27 §Synthesis updates and Change 3.2). What the CLI does own is (1) the
// per-slice override surface on `plan.yaml` (Change 2.3, pinned in
// `tests/plan_orchestrate.rs::plan_amend_authority_override_round_trips_and_validates`),
// (2) the per-Evidence override map on Evidence (Change 1.1 schema delta),
// (3) the byte-stable `fusion.yaml` schema (`schemas/slice/fusion.schema.json`)
// and round-trip / inspection paths (Change 2.6), and (4) the four-step
// resolution micro-resolver pin at
// `crates/domain/src/evidence/authority.rs::tests::resolution_order_*`.
//
// The acceptance test below ties those four together: it stages a slice whose
// `plan.yaml` carries `authority-override: { criterion: runtime }`, drops in a
// hand-authored `fusion.yaml` representing the synthesis output the resolver
// described above WOULD produce, and asserts `specify slice fusion show`
// faithfully surfaces the resolution-trace shape the operator audits.
// ---------------------------------------------------------------------------

const PLAN_WITH_PER_SLICE_OVERRIDE: &str = "\
name: rfc27-26-2
lifecycle: pending
sources:
  identity-design-notes: ./design-notes
  runtime: ./fixtures/replay
slices:
  - name: my-slice
    status: pending
    sources:
      - { key: identity-design-notes, candidate: my-slice }
      - { key: runtime, candidate: my-slice }
    authority-override:
      criterion: runtime
";

// `fusion.yaml` the synthesis resolver WOULD produce on the inputs above when
// docs and runtime disagree on a `criterion` claim and the per-slice override
// flips precedence to behaviour. Hand-authored so the test pins the shape the
// skill body must emit when Change 3.2 lands.
const FUSION_WITH_PER_SLICE_OVERRIDE_TRACE: &str = "version: 1
slice: my-slice
generated-at: 2026-05-22T13:15:00Z
generator: specify@2.1.0
requirements:
  - id: REQ-007
    status: divergence
    sources: [identity-design-notes, runtime]
    contributing-claims:
      - source: runtime
        claim-id: password-reset.expiry
        kind: criterion
        value: \"expiresAt = createdAt + 24h\"
        path: tests/data/replay/password-reset/expiry.json
        winner: true
      - source: identity-design-notes
        claim-id: password-reset.expiry
        kind: criterion
        value: \"Reset links expire after 30 minutes.\"
        path: docs/account.md#L7
        winner: false
    resolution: per-slice-override
    resolution-trace:
      step: per-slice-authority-override
      override:
        criterion: runtime
      winner: runtime
";

const SPEC_WITH_PER_SLICE_OVERRIDE: &str = "### Requirement: Password reset request expiry

ID: REQ-007
Sources: [identity-design-notes, runtime]
Status: divergence

Reset links expire after 24 hours (runtime fixture wins per per-slice override).
";

#[test]
fn fusion_show_round_trips_per_slice_authority_override_trace() {
    // Release blocker #26-2: the `per-slice-authority-override` trace must
    // survive a round-trip through the fusion loader + JSON renderer so the
    // operator audit path sees the resolution-trace step verbatim.
    let project =
        stage_slice_with_spec(SPEC_WITH_PER_SLICE_OVERRIDE, Some(PLAN_WITH_PER_SLICE_OVERRIDE));
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("fusion.yaml"), FUSION_WITH_PER_SLICE_OVERRIDE_TRACE)
        .expect("write fusion.yaml");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "fusion", "show", "my-slice"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    let req = &value["requirements"][0];
    assert_eq!(req["id"], "REQ-007");
    assert_eq!(req["status"], "divergence");
    assert_eq!(req["resolution"], "per-slice-override");

    let trace = &req["resolution-trace"];
    assert_eq!(
        trace["step"], "per-slice-authority-override",
        "RFC-27 §Acceptance #26-2 demands the resolution-trace step reads `per-slice-authority-override`, got: {trace:#?}"
    );
    assert_eq!(trace["winner"], "runtime");
    assert_eq!(
        trace["override"]["criterion"], "runtime",
        "the override map must echo the per-slice authority-override directive, got: {trace:#?}"
    );

    let claims = req["contributing-claims"].as_array().expect("contributing-claims array");
    assert_eq!(claims.len(), 2, "both contributors must be preserved for operator audit");
    let winner = claims.iter().find(|c| c["winner"] == true).expect("winning claim");
    let loser = claims.iter().find(|c| c["winner"] == false).expect("losing claim");
    assert_eq!(winner["source"], "runtime", "behaviour-class runtime must win this slice");
    assert_eq!(
        loser["source"], "identity-design-notes",
        "the documentation claim must be preserved as commentary"
    );
    assert_eq!(
        loser["value"], "Reset links expire after 30 minutes.",
        "dropped value must survive inline in fusion.yaml so the operator does not need to open evidence/*.yaml"
    );
}

#[test]
fn fusion_show_round_trips_per_slice_authority_override_trace_text() {
    // The text rendering is the human-readable inspection path; the JSON
    // path covers byte-stable machine consumption. Both must surface the
    // resolution-trace step so an operator running `specify slice fusion show`
    // without `--format json` still sees the audit-relevant trace fields.
    let project =
        stage_slice_with_spec(SPEC_WITH_PER_SLICE_OVERRIDE, Some(PLAN_WITH_PER_SLICE_OVERRIDE));
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("fusion.yaml"), FUSION_WITH_PER_SLICE_OVERRIDE_TRACE)
        .expect("write fusion.yaml");
    let assert = specify()
        .current_dir(project.root())
        .args(["slice", "fusion", "show", "my-slice"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(
        stdout.contains("resolution: per-slice-override"),
        "expected resolution line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("per-slice-authority-override"),
        "expected resolution-trace step to render, got:\n{stdout}"
    );
    assert!(
        stdout.contains("runtime :: password-reset.expiry"),
        "expected contributing-claim line for the runtime winner, got:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// RFC-27 §Acceptance scenario #26-3 — per-Evidence `authority-overrides` only
// (no per-slice override).
//
// The Evidence document carries `authority-overrides: { decision: documentation }`;
// synthesis resolves `decision`-class disagreement via the per-Evidence override
// and falls back to RFC-25 default ordering for `requirement`-class
// disagreements. `fusion.yaml` records BOTH resolution paths — the
// `authority-resolved` resolution with `step: per-evidence-authority-override`
// for the decision-kind claim, and `authority-resolved` with
// `step: default-authority-ordering` for the requirement-kind claim.
// ---------------------------------------------------------------------------

const PLAN_WITH_EVIDENCE_ONLY_OVERRIDE: &str = "\
name: rfc27-26-3
lifecycle: pending
sources:
  identity-design-notes: ./design-notes
  legacy-monolith: ./legacy
slices:
  - name: my-slice
    status: pending
    sources:
      - { key: identity-design-notes, candidate: my-slice }
      - { key: legacy-monolith, candidate: my-slice }
";

const FUSION_WITH_TWO_RESOLUTION_PATHS: &str = "version: 1
slice: my-slice
generated-at: 2026-05-22T13:15:00Z
generator: specify@2.1.0
requirements:
  - id: REQ-001
    status: divergence
    sources: [identity-design-notes, legacy-monolith]
    contributing-claims:
      - source: identity-design-notes
        claim-id: account.reset-link-decision
        kind: decision
        value: \"Reset links are signed JWTs scoped to user-id + nonce.\"
        path: docs/account.md#L42
        winner: true
      - source: legacy-monolith
        claim-id: account.reset-link-decision
        kind: decision
        value: \"reset-link uses opaque token in users.reset_tokens table\"
        path: src/users/reset.ts#L18
        winner: false
    resolution: authority-resolved
    resolution-trace:
      step: per-evidence-authority-override
      override:
        decision: documentation
      winner: identity-design-notes
  - id: REQ-002
    status: divergence
    sources: [identity-design-notes, legacy-monolith]
    contributing-claims:
      - source: identity-design-notes
        claim-id: account.reset-request
        kind: requirement
        value: \"A registered user may request a reset link by email.\"
        path: docs/account.md#L7
        winner: true
      - source: legacy-monolith
        claim-id: account.reset-request
        kind: requirement
        value: \"requestReset(email) issues a token in the reset_tokens table.\"
        path: src/users/reset.ts#L30
        winner: false
    resolution: authority-resolved
    resolution-trace:
      step: default-authority-ordering
      winner: identity-design-notes
";

const SPEC_WITH_TWO_RESOLUTION_PATHS: &str = "### Requirement: Reset link decision

ID: REQ-001
Sources: [identity-design-notes, legacy-monolith]
Status: divergence

Reset links are signed JWTs; legacy opaque-token implementation preserved as commentary.

### Requirement: Reset link request

ID: REQ-002
Sources: [identity-design-notes, legacy-monolith]
Status: divergence

A registered user may request a reset link by email.
";

#[test]
fn fusion_show_records_both_per_evidence_and_default_authority_paths() {
    // RFC-27 §Acceptance #26-3: `fusion.yaml` records BOTH resolution paths
    // — the per-Evidence override for the `decision`-kind disagreement AND
    // the default-authority-ordering fallback for the `requirement`-kind
    // disagreement — on the same slice without any per-slice override
    // intervening.
    let project = stage_slice_with_spec(
        SPEC_WITH_TWO_RESOLUTION_PATHS,
        Some(PLAN_WITH_EVIDENCE_ONLY_OVERRIDE),
    );
    let slice_dir = project.slices_dir().join("my-slice");
    fs::write(slice_dir.join("fusion.yaml"), FUSION_WITH_TWO_RESOLUTION_PATHS)
        .expect("write fusion.yaml");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "fusion", "show", "my-slice"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    let requirements = value["requirements"].as_array().expect("requirements array");
    assert_eq!(requirements.len(), 2, "both resolution paths must be present");

    let req1 = requirements.iter().find(|r| r["id"] == "REQ-001").expect("REQ-001 present");
    assert_eq!(req1["resolution"], "authority-resolved");
    let trace1 = &req1["resolution-trace"];
    assert_eq!(
        trace1["step"], "per-evidence-authority-override",
        "decision-kind disagreement must resolve through the per-Evidence override step"
    );
    assert_eq!(trace1["override"]["decision"], "documentation");

    let req2 = requirements.iter().find(|r| r["id"] == "REQ-002").expect("REQ-002 present");
    assert_eq!(req2["resolution"], "authority-resolved");
    let trace2 = &req2["resolution-trace"];
    assert_eq!(
        trace2["step"], "default-authority-ordering",
        "requirement-kind disagreement must fall back to RFC-25 default ordering when no override fires"
    );
    // Documentation outranks behaviour per RFC-25; identity-design-notes is the
    // documentation-authority contributor and must win the default-ordering path.
    assert_eq!(trace2["winner"], "identity-design-notes");
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
