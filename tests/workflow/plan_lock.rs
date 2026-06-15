//! Dual-driving refusal: the plan-state-writing verbs
//! (`plan next`, per-entry `plan transition`, `slice merge run`)
//! probe `<plan-root>/.specify/plan.lock` and refuse an unlocked
//! driver with `plan-lock-not-held` (exit 2). Acquisition stays
//! skill-side (plan-lock.md); these tests stand in for the driver
//! session with `Project::hold_plan_lock`. This file is the named
//! CLI-test replacement for the retired `dual-driving-refused` eval
//! scenario.

use crate::support::*;

#[test]
fn gated_verbs_refuse_unlocked_driver() {
    // One case per gated verb: (seed, argv, status line that must
    // survive). The surviving status proves the refusal wrote no plan
    // state; the journal must stay absent because the lock probe fires
    // before any event bracket (`plan.entry.advanced`, the
    // `slice.merge.*` pair). The merge slice doesn't even need to exist
    // for the refusal path.
    let cases: [(&str, &[&str], &str); 4] = [
        (SINGLE_PENDING, &["plan", "next"], "status: pending"),
        (A_IN_PROGRESS, &["plan", "transition", "a", "done"], "status: in-progress"),
        (ALL_DONE, &["plan", "transition", "a", "--undo"], "status: done"),
        (A_IN_PROGRESS, &["slice", "merge", "run", "a"], "status: in-progress"),
    ];

    for (seed, args, surviving_status) in cases {
        let project = Project::init();
        project.seed_plan(seed);

        let assert = specify_cmd()
            .current_dir(project.root())
            .args(["--format", "json"])
            .args(args)
            .assert()
            .failure();
        assert_eq!(
            assert.get_output().status.code(),
            Some(2),
            "{args:?}: lock refusal is a validation exit"
        );
        let stderr = parse_stderr(&assert.get_output().stderr, project.root());
        assert_eq!(stderr["error"], "plan-lock-not-held", "{args:?}: stderr envelope: {stderr}");

        let plan = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
        assert!(
            plan.contains(surviving_status),
            "{args:?}: refusal must not write plan state: {plan}"
        );
        assert!(
            !project.root().join(".specify/journal.jsonl").exists(),
            "{args:?}: refusal must not journal"
        );
    }
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
