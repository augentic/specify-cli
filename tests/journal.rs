//! Integration tests for the RFC-25 §Observability journal events.
//!
//! Verifies that each CLI-owned emit site writes the documented wire
//! shape into `.specify/journal.jsonl` and that the agent-facing
//! [`specify_domain::journal::append`] helper can be driven directly
//! from a synthesised event. Golden files under
//! `tests/fixtures/journal/` pin the canonical JSON-line shape; rerun
//! with `REGENERATE_GOLDENS=1 cargo test --test journal` to refresh.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use specify_domain::config::Layout;
use specify_domain::journal::{self, DivergenceState, Event, EventKind};

mod common;
use common::{Project, assert_golden_at, repo_root, specify};

/// Pinned RFC-3339 timestamp used by every golden snapshot. CLI-driven
/// emits use `Timestamp::now()`; tests normalise the value to this
/// placeholder before diffing so goldens stay deterministic.
const FIXED_TIMESTAMP: &str = "2026-05-21T20:00:00Z";

fn fixtures_dir() -> PathBuf {
    repo_root().join("tests/fixtures/journal")
}

fn journal_path(project_root: &Path) -> PathBuf {
    project_root.join(".specify").join("journal.jsonl")
}

/// Read `.specify/journal.jsonl`, return one `Value` per line. Strips
/// trailing blank lines so the asserted shape matches the golden
/// regardless of writer quirks.
fn read_journal(project_root: &Path) -> Vec<Value> {
    let raw = fs::read_to_string(journal_path(project_root))
        .unwrap_or_else(|err| panic!("read journal.jsonl: {err}"));
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("journal line is JSON"))
        .collect()
}

/// Normalise every event's `timestamp` to [`FIXED_TIMESTAMP`] so the
/// golden snapshot is stable across runs.
fn normalise_timestamps(mut events: Vec<Value>) -> Vec<Value> {
    for event in &mut events {
        if let Value::Object(map) = event
            && map.contains_key("timestamp")
        {
            map.insert("timestamp".to_string(), Value::String(FIXED_TIMESTAMP.to_string()));
        }
    }
    events
}

fn assert_journal_golden(name: &str, events: Vec<Value>) {
    let actual = Value::Array(normalise_timestamps(events));
    assert_golden_at(&fixtures_dir(), name, actual);
}

// -- plan.transition.reviewed ----------------------------------------

#[test]
fn plan_transition_reviewed_emits_journal_event() {
    let project = Project::init();
    project.seed_plan(
        "name: platform-v2
slices:
  - name: a
    project: default
    status: done
",
    );

    specify()
        .current_dir(project.root())
        .args(["plan", "transition", "platform-v2", "reviewed"])
        .assert()
        .success();

    let events = read_journal(project.root());
    assert_eq!(events.len(), 1, "expected one journal event, got {}", events.len());
    assert_eq!(events[0]["event"], "plan.transition.reviewed");
    assert_eq!(events[0]["payload"]["plan-name"], "platform-v2");
    assert!(
        events[0]["timestamp"].as_str().is_some(),
        "timestamp must be present, got:\n{}",
        events[0]
    );
    assert_journal_golden("plan-transition-reviewed.json", events);
}

// -- plan.amend.divergence -------------------------------------------

const TWO_SLICE_PLAN: &str = "\
name: platform-v2
slices:
  - name: checkout
    project: default
    status: pending
  - name: billing
    project: default
    status: pending
";

#[test]
fn plan_amend_divergence_from_none_to_accepted() {
    // RFC-25 subagent note: the implicit-default first transition
    // serialises `from: none` because the on-disk slice has no
    // `divergence:` key.
    let project = Project::init();
    project.seed_plan(TWO_SLICE_PLAN);

    specify()
        .current_dir(project.root())
        .args(["plan", "amend", "checkout", "--divergence", "accepted"])
        .assert()
        .success();

    let events = read_journal(project.root());
    assert_eq!(events.len(), 1);
    let payload = &events[0]["payload"];
    assert_eq!(events[0]["event"], "plan.amend.divergence");
    assert_eq!(payload["plan-name"], "platform-v2");
    assert_eq!(payload["slice-name"], "checkout");
    assert_eq!(payload["from"], "none");
    assert_eq!(payload["to"], "accepted");
    assert_journal_golden("plan-amend-divergence-none-to-accepted.json", events);
}

#[test]
fn plan_amend_divergence_from_likely_to_rejected() {
    // RFC-25 subagent note: `propose` writes `divergence: likely` and
    // the operator may transition it to `rejected` at Gate 1.
    let project = Project::init();
    project.seed_plan(
        "name: platform-v2
slices:
  - name: checkout
    project: default
    status: pending
    divergence: likely
",
    );

    specify()
        .current_dir(project.root())
        .args(["plan", "amend", "checkout", "--divergence", "rejected"])
        .assert()
        .success();

    let events = read_journal(project.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"], "plan.amend.divergence");
    assert_eq!(events[0]["payload"]["from"], "likely");
    assert_eq!(events[0]["payload"]["to"], "rejected");
    assert_journal_golden("plan-amend-divergence-likely-to-rejected.json", events);
}

#[test]
fn plan_amend_divergence_from_accepted_to_rejected() {
    let project = Project::init();
    project.seed_plan(
        "name: platform-v2
slices:
  - name: checkout
    project: default
    status: pending
    divergence: accepted
",
    );

    specify()
        .current_dir(project.root())
        .args(["plan", "amend", "checkout", "--divergence", "rejected"])
        .assert()
        .success();

    let events = read_journal(project.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["payload"]["from"], "accepted");
    assert_eq!(events[0]["payload"]["to"], "rejected");
    assert_journal_golden("plan-amend-divergence-accepted-to-rejected.json", events);
}

#[test]
fn plan_amend_divergence_from_rejected_to_accepted() {
    let project = Project::init();
    project.seed_plan(
        "name: platform-v2
slices:
  - name: checkout
    project: default
    status: pending
    divergence: rejected
",
    );

    specify()
        .current_dir(project.root())
        .args(["plan", "amend", "checkout", "--divergence", "accepted"])
        .assert()
        .success();

    let events = read_journal(project.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["payload"]["from"], "rejected");
    assert_eq!(events[0]["payload"]["to"], "accepted");
    assert_journal_golden("plan-amend-divergence-rejected-to-accepted.json", events);
}

#[test]
fn plan_amend_without_divergence_flag_emits_no_event() {
    let project = Project::init();
    project.seed_plan(TWO_SLICE_PLAN);

    specify()
        .current_dir(project.root())
        .args(["plan", "amend", "checkout", "--description", "scope hint"])
        .assert()
        .success();

    assert!(
        !journal_path(project.root()).exists(),
        "amends without --divergence must not write a journal event"
    );
}

// -- slice.transition.refined ----------------------------------------

#[test]
fn slice_transition_to_defined_does_not_emit_refined_event() {
    // RFC-25 §Observability emits only on the `refined` target — the
    // 1.x lifecycle transitions (defined / building / complete) are
    // not part of the v1 event set.
    let project = Project::init();
    specify().current_dir(project.root()).args(["slice", "create", "checkout"]).assert().success();
    specify()
        .current_dir(project.root())
        .args(["slice", "transition", "checkout", "defined"])
        .assert()
        .success();

    assert!(
        !journal_path(project.root()).exists(),
        "non-refined lifecycle transitions must not produce journal events"
    );
}

#[test]
fn slice_transition_refined_happy_path_via_emit_helper() {
    // RFC-25 §Observability lists `slice.transition.refined` as the
    // synthesis-completed signal. The CLI verb
    // `specify slice transition <name> refined` will emit this event
    // once the lifecycle gains the `refined` state; until then the
    // happy-path golden is driven directly via the public emit
    // helper, which is the same surface W3.1 will call from
    // `/spec:refine`.
    let project = Project::init();
    let layout = Layout::new(project.root());
    let fixed: jiff::Timestamp =
        FIXED_TIMESTAMP.parse().expect("fixed timestamp parses as rfc3339");
    journal::append(
        layout,
        &Event::new(
            fixed,
            EventKind::SliceTransitionRefined {
                slice_name: "checkout".to_string(),
            },
        ),
    )
    .expect("append helper succeeds");

    let events = read_journal(project.root());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"], "slice.transition.refined");
    assert_eq!(events[0]["payload"]["slice-name"], "checkout");
    assert_journal_golden("slice-transition-refined.json", events);
}

// -- agent-emit helper (slice.synthesis.*, slice.extract.completed,
// -- plan.propose.divergence) ---------------------------------------

#[test]
fn agent_emit_helper_writes_one_event_per_jsonl_line() {
    // Exercises the public Rust helper W3.1 / W3.2 skill bodies call
    // for the agent-driven events. The harness drives `append`
    // directly because the CLI does not own a `journal append` verb
    // (RFC-25 §"What was cut and why").
    let project = Project::init();
    let layout = Layout::new(project.root());
    let fixed: jiff::Timestamp =
        FIXED_TIMESTAMP.parse().expect("fixed timestamp parses as rfc3339");

    let events = [
        Event::new(
            fixed,
            EventKind::PlanProposeDivergence {
                plan_name: "platform-v2".to_string(),
                slice_name: "checkout".to_string(),
            },
        ),
        Event::new(
            fixed,
            EventKind::SliceExtractCompleted {
                slice_name: "checkout".to_string(),
                source_key: "monolith".to_string(),
            },
        ),
        Event::new(
            fixed,
            EventKind::SliceSynthesisConflict {
                slice_name: "checkout".to_string(),
                requirement_id: "R-01".to_string(),
            },
        ),
        Event::new(
            fixed,
            EventKind::SliceSynthesisDivergence {
                slice_name: "checkout".to_string(),
                requirement_id: "R-02".to_string(),
            },
        ),
        Event::new(
            fixed,
            EventKind::SliceSynthesisUnknown {
                slice_name: "checkout".to_string(),
                requirement_id: "R-03".to_string(),
            },
        ),
    ];
    for event in &events {
        journal::append(layout, event).expect("append helper succeeds");
    }

    let raw = fs::read_to_string(journal_path(project.root())).expect("read journal");
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 5, "expected five JSON lines, got {}", lines.len());
    for line in &lines {
        let parsed: Map<String, Value> = serde_json::from_str(line).expect("each line is JSON");
        assert!(parsed.contains_key("timestamp"), "line missing timestamp: {line}");
        assert!(parsed.contains_key("event"), "line missing event id: {line}");
        assert!(parsed.contains_key("payload"), "line missing payload: {line}");
    }

    let values: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_journal_golden("agent-emit-helper.json", values);
}

#[test]
fn divergence_state_kebab_case_round_trip() {
    // Wire-format guard: snake_case lifecycle values are never
    // produced anywhere on disk (RFC-25 §Wire format).
    for state in [
        DivergenceState::None,
        DivergenceState::Likely,
        DivergenceState::Accepted,
        DivergenceState::Rejected,
    ] {
        let rendered = serde_json::to_string(&state).expect("DivergenceState serialises");
        assert!(
            !rendered.contains('_'),
            "DivergenceState `{state:?}` must not contain `_` on the wire; got {rendered}"
        );
    }
}
