//! RFC-44 R2 dual-driving refusal: the plan-state-writing verbs
//! (`plan next`, per-entry `plan transition`, `slice merge run`)
//! probe `<plan-root>/.specify/plan.lock` and refuse an unlocked
//! driver with `plan-lock-not-held` (exit 2). Acquisition stays
//! skill-side (plan-lock.md); these tests stand in for the driver
//! session with `Project::hold_plan_lock`. This file is the named
//! CLI-test replacement for the retired `dual-driving-refused` eval
//! scenario.

use assert_cmd::assert::Assert;

use crate::support::*;

/// Assert the command refused with `plan-lock-not-held` on exit 2.
fn assert_lock_refused(assert: Assert, project: &Project) {
    let assert = assert.failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "lock refusal is a validation exit");
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "plan-lock-not-held", "stderr envelope: {stderr}");
}

#[test]
fn next_refuses_unlocked_driver() {
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert();
    assert_lock_refused(assert, &project);

    let plan = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(plan.contains("status: pending"), "refusal must not advance the entry: {plan}");
    assert!(
        !project.root().join(".specify/journal.jsonl").exists(),
        "refusal must not journal plan.entry.advanced"
    );
}

#[test]
fn transition_done_refuses_unlocked_driver() {
    let project = Project::init();
    project.seed_plan(A_IN_PROGRESS);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "a", "done"])
        .assert();
    assert_lock_refused(assert, &project);

    let plan = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(plan.contains("status: in-progress"), "refusal must not stamp done: {plan}");
}

#[test]
fn transition_undo_refuses_unlocked_driver() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "a", "--undo"])
        .assert();
    assert_lock_refused(assert, &project);
}

#[test]
fn merge_run_refuses_unlocked_driver() {
    // The probe fires before the `slice.merge.*` bracket, so a refusal
    // leaves no merge events behind — the slice doesn't even need to
    // exist for the refusal path.
    let project = Project::init();
    project.seed_plan(A_IN_PROGRESS);

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "merge", "run", "a"])
        .assert();
    assert_lock_refused(assert, &project);
    assert!(
        !project.root().join(".specify/journal.jsonl").exists(),
        "a lock refusal must not journal slice.merge.started/failed"
    );
}

#[test]
fn gate_one_approved_is_exempt() {
    // The plan-level Gate 1 stamp precedes any driver session — it must
    // succeed without the lock.
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    specify_cmd()
        .current_dir(project.root())
        .args(["plan", "transition", "demo", "approved"])
        .assert()
        .success();
}

#[test]
fn gated_verbs_pass_under_lock() {
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);
    let _lock = project.hold_plan_lock();

    specify_cmd().current_dir(project.root()).args(["plan", "next"]).assert().success();
    specify_cmd()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "done"])
        .assert()
        .success();
}
