//! Plan lifecycle CLI tests: validate, next, add/remove/amend,
//! transition, human-driven replay, and `--auto-approve`.

use crate::support::*;

// -- validate ----------------------------------------------------------

#[test]
fn plan_validate_clean_text() {
    let project = Project::init();
    project.seed_plan(CLEAN_PLAN);

    let assert =
        specrun().current_dir(project.root()).args(["plan", "validate"]).assert().success();
    assert_eq!(assert.get_output().status.code(), Some(0));

    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    // No ERROR-level lines on a clean plan.
    assert!(!stdout.contains("ERROR"), "clean plan must not print any ERROR lines, got:\n{stdout}");
}

#[test]
fn plan_validate_clean_json() {
    let project = Project::init();
    project.seed_plan(CLEAN_PLAN);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "validate"])
        .assert()
        .success();
    assert_eq!(assert.get_output().status.code(), Some(0));

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["passed"], true);
    assert_eq!(actual["results"], Value::Array(vec![]));
    assert_golden("validate-clean.json", actual);
}

#[test]
fn plan_validate_tolerates_in_progress() {
    // Transient window: `specify change transition <name> in-progress`
    // can run a moment before `.specify/slices/<name>/` exists.
    // `specrun plan validate` must surface a *warning* (not an
    // error) so `passed == true` and skills don't stall on start-up.
    let project = Project::init();
    project.seed_plan(A_IN_PROGRESS);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "validate"])
        .assert()
        .success();
    assert_eq!(
        assert.get_output().status.code(),
        Some(0),
        "warning-only validate must exit 0 (EXIT_SUCCESS)"
    );

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(
        actual["passed"], true,
        "in-progress-without-slice-dir is a warning, so passed must be true: {actual}"
    );
    let results = actual["results"].as_array().expect("results array");
    let matching: Vec<&Value> =
        results.iter().filter(|r| r["code"] == "missing-slice-dir-for-in-progress").collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one missing-slice-dir-for-in-progress result, got: {results:#?}"
    );
    assert_eq!(matching[0]["severity"], "warning");
    assert_eq!(matching[0]["entry"], "a");
}

#[test]
fn plan_validate_with_errors_json() {
    let project = Project::init();
    project.seed_plan(DUPLICATE_NAME_PLAN);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "validate"])
        .assert()
        .failure();
    assert_eq!(
        assert.get_output().status.code(),
        Some(2),
        "duplicate-name must exit 2 (EXIT_VALIDATION_FAILED)"
    );

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["passed"], false);
    let results = actual["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["code"] == "duplicate-name" && r["severity"] == "error"),
        "expected a duplicate-name error, got: {results:#?}"
    );
    assert_golden("validate-duplicate-name.json", actual);
}

// -- next --------------------------------------------------------------

#[test]
fn plan_next_picks_first_pending_text() {
    let project = Project::init();
    project.seed_plan(A_DONE_B_PENDING);

    let assert = specrun().current_dir(project.root()).args(["plan", "next"]).assert().success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert_eq!(stdout, "b\n", "text next should be bare '<name>\\n', got: {stdout:?}");
}

#[test]
fn plan_next_picks_first_pending_json() {
    let project = Project::init();
    project.seed_plan(A_DONE_B_PENDING);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["next"], "b");
    assert_eq!(actual["reason"], Value::Null);
    assert_eq!(actual["active"], Value::Null);
    assert_eq!(actual["project"], "default", "project should match seeded value");
    assert_eq!(actual["description"], Value::Null, "description should be present");
    assert!(
        actual.get("sources").is_some(),
        "sources field should be present in plan next response"
    );
    assert_golden("next-first-pending.json", actual);
}

#[test]
fn plan_next_reports_in_progress() {
    let project = Project::init();
    project.seed_plan(A_IN_PROGRESS);

    let text = specrun().current_dir(project.root()).args(["plan", "next"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert!(stdout.contains('a'), "text output should mention 'a': {stdout:?}");

    let json = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    let actual = parse_stdout(&json.get_output().stdout, project.root());
    assert_eq!(actual["next"], Value::Null);
    assert_eq!(actual["reason"], "in-progress");
    assert_eq!(actual["active"], "a");
    assert_golden("next-in-progress.json", actual);
}

#[test]
fn plan_next_all_done_text() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);

    let text = specrun().current_dir(project.root()).args(["plan", "next"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert!(stdout.contains("drained"), "drained text should mention drained, got: {stdout:?}");

    let json = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    let actual = parse_stdout(&json.get_output().stdout, project.root());
    assert_eq!(actual["reason"], "drained");
    assert_eq!(actual["next"], Value::Null);
    assert_eq!(actual["active"], Value::Null);
    assert_golden("next-all-done.json", actual);
}

#[test]
fn plan_next_stuck_when_deps_unmet() {
    let project = Project::init();
    project.seed_plan(STUCK_PLAN);

    let json = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    let actual = parse_stdout(&json.get_output().stdout, project.root());
    assert_eq!(
        actual["reason"], "drained",
        "post-2.0 the legacy `stuck` fixture is now drained (all-done)"
    );
    assert_eq!(actual["next"], Value::Null);
    assert_eq!(actual["active"], Value::Null);
    assert_golden("next-stuck.json", actual);
}

// -- create / amend / transition (L1.J write-side commands) -----------

const EMPTY_PLAN: &str = "\
name: demo
slices: []
";

const SINGLE_PENDING: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: pending
";

const SINGLE_IN_PROGRESS: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: in-progress
";

const SINGLE_DONE: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: done
";

const WITH_DESCRIPTION: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: pending
    description: original
";

// -- plan add ---------------------------------------------------------

#[test]
fn plan_add_appends_pending_entry_json() {
    let project = Project::init();
    project.seed_plan(EMPTY_PLAN);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "add", "foo"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["action"], "create");
    assert_eq!(actual["entry"]["name"], "foo");
    assert_eq!(actual["entry"]["status"], "pending");
    assert_eq!(actual["entry"]["status-reason"], Value::Null);
    assert_eq!(actual["plan"]["name"], "demo");

    let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(saved.contains("name: foo"), "saved plan missing new entry:\n{saved}");
    assert!(saved.contains("status: pending"), "saved plan missing pending status:\n{saved}");

    assert_golden("create-foo.json", actual);
}

#[test]
fn plan_add_rejects_duplicate_name_text() {
    let project = Project::init();
    project.seed_plan(EMPTY_PLAN);

    specrun().current_dir(project.root()).args(["plan", "add", "foo"]).assert().success();

    let assert =
        specrun().current_dir(project.root()).args(["plan", "add", "foo"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("already contains a change"),
        "stderr should flag duplicate, got: {stderr:?}"
    );
}

#[test]
fn plan_add_rejects_invalid_name() {
    let project = Project::init();
    project.seed_plan(EMPTY_PLAN);

    let assert =
        specrun().current_dir(project.root()).args(["plan", "add", "NotKebab"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));

    let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(!saved.contains("NotKebab"), "invalid name must not land in the plan:\n{saved}");
}

// -- plan remove ------------------------------------------------------

#[test]
fn plan_remove_drops_pending_entry() {
    let project = Project::init();
    project.seed_plan(
        "\
name: demo
slices:
  - name: a
    project: default
    status: pending
  - name: b
    project: default
    status: pending
",
    );

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "remove", "a"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["action"], "remove");
    assert_eq!(actual["entry"]["name"], "a");

    let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(!saved.contains("name: a"), "removed entry must not remain:\n{saved}");
    assert!(saved.contains("name: b"), "other entry must remain:\n{saved}");
}

#[test]
fn plan_remove_refuses_when_depended_on() {
    let project = Project::init();
    project.seed_plan(
        "\
name: demo
slices:
  - name: a
    project: default
    status: pending
  - name: b
    project: default
    status: pending
    depends-on: [a]
",
    );

    let assert =
        specrun().current_dir(project.root()).args(["plan", "remove", "a"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("plan-remove-entry-referenced"),
        "stderr should name the validation code, got: {stderr:?}"
    );
}

// -- plan amend -------------------------------------------------------

#[test]
fn plan_amend_replaces_depends_on() {
    let project = Project::init();
    project.seed_plan(
        "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: done
  - name: foo
    project: default
    status: pending
    depends-on: [a]
",
    );

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "amend",
            "foo",
            "--depends-on",
            "a",
            "--depends-on",
            "b",
        ])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["action"], "amend");
    assert_eq!(actual["entry"]["name"], "foo");
    let deps = actual["entry"]["depends-on"].as_array().expect("deps array");
    let names: Vec<&str> = deps.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(names, ["a", "b"]);

    assert_golden("amend-replace-depends-on.json", actual);

    let saved = fs::read_to_string(project.plan_path()).expect("read");
    assert!(saved.contains("- a"), "saved depends-on missing 'a':\n{saved}");
    assert!(saved.contains("- b"), "saved depends-on missing 'b':\n{saved}");
}

#[test]
fn plan_amend_clears_description() {
    let project = Project::init();
    project.seed_plan(WITH_DESCRIPTION);

    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--description", ""])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read");
    assert!(
        !saved.contains("description: original"),
        "original description should be gone:\n{saved}"
    );
}

#[test]
fn plan_amend_leaves_field_alone() {
    let project = Project::init();
    project.seed_plan(WITH_DESCRIPTION);

    // --depends-on (clear) but no --description; description must stay.
    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--depends-on"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read");
    assert!(saved.contains("description: original"), "description should be preserved:\n{saved}");
}

#[test]
fn plan_amend_on_missing_entry_fails() {
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "nope", "--description", "x"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(
        stderr.contains("no slice named"),
        "stderr should mention missing change, got: {stderr:?}"
    );
}

// -- plan transition --------------------------------------------------

#[test]
fn plan_transition_happy_path_text() {
    // Post-2.0 the only legal per-entry transition is
    // `InProgress -> Done`. We pre-stage `in-progress` via `plan next`
    // (the only writer of `in-progress`) and then close the entry.
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    specrun().current_dir(project.root()).args(["plan", "next"]).assert().success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "done"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert!(stdout.contains("in-progress"), "text output should mention 'in-progress': {stdout:?}");
    assert!(stdout.contains("done"), "text output should mention 'done': {stdout:?}");
}

#[test]
fn plan_transition_legal_edge_json() {
    let project = Project::init();
    project.seed_plan(SINGLE_IN_PROGRESS);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "foo", "done"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["name"], "foo");
    assert_eq!(actual["current"], "done");
    assert_eq!(actual["previous"], "in-progress");
    assert_eq!(actual["kind"], "entry");

    assert_golden("transition-in-progress-to-done.json", actual);
}

#[test]
fn plan_transition_rejects_illegal_edge() {
    let project = Project::init();
    project.seed_plan(SINGLE_DONE);

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "pending"])
        .assert()
        .failure();
    let code = assert.get_output().status.code();
    assert!(
        code == Some(1) || code == Some(2),
        "illegal transition should be rejected (exit 1 or 2), got: {code:?}"
    );
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(
        stderr.to_lowercase().contains("transition")
            || stderr.contains("plan add")
            || stderr.contains("plan next")
            || stderr.contains("argument"),
        "stderr should mention the rejected transition, got: {stderr:?}"
    );
}

#[test]
fn transition_undo_done_to_in_progress() {
    let project = Project::init();
    project.seed_plan(SINGLE_DONE);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "foo", "--undo"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["kind"], "undo");
    assert_eq!(actual["name"], "foo");
    assert_eq!(actual["previous"], "done");
    assert_eq!(actual["current"], "in-progress");
    assert_eq!(actual["undo"]["from"], "done");
    assert_eq!(actual["undo"]["to"], "in-progress");

    let plan_after = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(plan_after.contains("status: in-progress"), "plan.yaml: {plan_after}");

    let journal = fs::read_to_string(project.root().join(".specify").join("journal.jsonl"))
        .expect("read journal.jsonl");
    let last = journal.lines().rfind(|l| !l.is_empty()).expect("journal line");
    assert!(
        last.contains(r#""event":"plan.transition.undone""#),
        "undo must emit plan.transition.undone, got:\n{last}"
    );
    assert!(last.contains(r#""from":"done""#), "from in payload: {last}");
    assert!(last.contains(r#""to":"in-progress""#), "to in payload: {last}");
}

#[test]
fn undo_in_progress_to_pending_refuses() {
    let project = Project::init();
    project.seed_plan(SINGLE_IN_PROGRESS);

    specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "--undo"])
        .assert()
        .success();

    let plan_mid = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(plan_mid.contains("status: pending"), "plan.yaml after first undo: {plan_mid}");

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "--undo"])
        .assert()
        .failure();
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(
        stderr.contains("pending"),
        "undo-from-pending stderr should mention `pending`, got: {stderr:?}"
    );
}

#[test]
fn transition_plan_level_approved() {
    // workflow §The plan gate: `specrun plan transition <plan-name>
    // approved` is the operator-stamped Gate 1 transition. The plan
    // name on the wire matches `plan.yaml.name`.
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "demo", "approved"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["kind"], "plan");
    assert_eq!(actual["name"], "demo");
    assert_eq!(actual["previous"], "pending");
    assert_eq!(actual["current"], "approved");

    assert_golden("transition-plan-approved.json", actual);
}

#[test]
fn transition_rejects_per_entry_in_progress() {
    // Per-entry `in-progress` is owned by `plan next`. `plan transition`
    // must reject the request with an argument-shape error (exit 2).
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "in-progress"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(stderr.contains("plan next"), "stderr should point at `plan next`, got: {stderr:?}");
}

#[test]
fn plan_transition_rejects_retired_states() {
    // `blocked`, `failed`, and `skipped` were retired in source/target adapter split.
    // Each must be rejected with the same argument-shape error.
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    for retired in ["blocked", "failed", "skipped"] {
        let assert = specrun()
            .current_dir(project.root())
            .args(["plan", "transition", "foo", retired])
            .assert()
            .failure();
        assert_eq!(
            assert.get_output().status.code(),
            Some(2),
            "retired target `{retired}` must yield exit 2"
        );
    }
}

// pre-2.0 `plan transition <name> failed --reason <text>` retired
// alongside the per-entry `failed` state — see
// `plan_transition_rejects_retired_states` above.

#[test]
fn transition_rejects_unknown_reason() {
    // `--reason` was retired in source/target adapter split (no v1 per-entry state accepts a
    // reason). Clap surfaces unknown flags as exit 2 with `--reason`
    // named in stderr.
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "in-progress", "--reason", "x"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(stderr.contains("--reason"), "stderr should mention '--reason', got: {stderr:?}");
}

// Re-entry to `pending` retired with the per-entry status purge
// (the 2.0 collapse removed the per-entry enum to `pending | in-progress | done`).

// -- human-driven replay (the human-driven plan loop) -----------

#[test]
fn plan_human_replay_matches_fixture() {
    let project = Project::init();
    project.seed_plan(
        "\
name: demo
slices:
  - name: user-registration
    project: default
    status: done
",
    );

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "registration-duplicate-email-crash",
            "--description",
            "Duplicate email submission returns 500 instead of 409. Modifies user-registration.",
        ])
        .assert()
        .success();

    specrun().current_dir(project.root()).args(["plan", "next"]).assert().success();

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "registration-duplicate-email-crash",
            "--description",
            "Clarified scope",
        ])
        .assert()
        .success();

    specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "registration-duplicate-email-crash", "done"])
        .assert()
        .success();

    let actual = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    let fixture_path = plan_fixtures().join("human-replay-final.yaml");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::create_dir_all(plan_fixtures()).expect("mkdir plan fixtures");
        fs::write(&fixture_path, &actual).expect("write fixture");
        return;
    }

    let expected = fs::read_to_string(&fixture_path).unwrap_or_else(|err| {
            panic!(
                "fixture {} missing ({err}); regenerate via REGENERATE_GOLDENS=1 cargo test --test plan_orchestrate",
                fixture_path.display()
            )
        });

    assert_eq!(
        actual,
        expected,
        "plan.yaml after replay diverged from fixture {}\n--- actual ---\n{actual}\n--- expected ---\n{expected}",
        fixture_path.display()
    );
}

#[test]
fn create_scaffolds_matches_golden() {
    let project = Project::init();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "create", "my-change"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["name"], "my-change");
    let plan_path = actual["plan"].as_str().expect("plan string");
    assert!(
        plan_path.ends_with("/plan.yaml"),
        "plan should end with /plan.yaml at the repo root, got: {plan_path}"
    );

    assert!(project.plan_path().exists(), "plan.yaml should be created");
    assert!(!project.root().join("change.md").exists(), "plan create must not write change.md");

    assert_golden("plan-create.json", actual);
}

#[test]
fn create_refuses_overwrite() {
    let project = Project::init();
    specrun().current_dir(project.root()).args(["plan", "create", "first"]).assert().success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "create", "second"])
        .assert()
        .failure();
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "already-exists");
}

#[test]
fn plan_create_then_validate_passes_clean() {
    let project = Project::init();

    specrun().current_dir(project.root()).args(["plan", "create", "fresh"]).assert().success();

    let assert =
        specrun().current_dir(project.root()).args(["plan", "validate"]).assert().success();
    assert_eq!(assert.get_output().status.code(), Some(0));
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert!(
        !stdout.contains("ERROR"),
        "freshly-scaffolded plan must pass `specrun plan validate` with no errors, got:\n{stdout}"
    );
}

// -- plan create --auto-approve (auto-approve Gate-1 contract) ---------------------------

#[test]
fn create_auto_approve_stamps() {
    // auto-approve Gate-1 contract: `--auto-approve` is the operator's Gate-1 consent at
    // create time. The on-disk plan carries `lifecycle: approved`
    // directly (single atomic write — no transient `pending`
    // observable to readers) and the journal carries exactly one
    // `plan.transition.approved` event matching the post-create stamp.
    let project = Project::init();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "create", "fresh", "--auto-approve"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["name"], "fresh");
    assert_eq!(actual["lifecycle"], "approved");

    let on_disk = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(
        on_disk.contains("lifecycle: approved"),
        "plan.yaml must carry `lifecycle: approved` after --auto-approve, got:\n{on_disk}"
    );
    assert!(
        !on_disk.contains("lifecycle: pending"),
        "no transient `lifecycle: pending` must remain on disk, got:\n{on_disk}"
    );

    let journal = project.root().join(".specify").join("journal.jsonl");
    let raw = fs::read_to_string(&journal).expect("read journal.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one journal event (plan.transition.approved) per --auto-approve create, got:\n{raw}"
    );
    assert!(
        lines[0].contains(r#""event":"plan.transition.approved""#),
        "first (and only) line must be plan.transition.approved, got:\n{}",
        lines[0]
    );
    assert!(
        lines[0].contains(r#""plan-name":"fresh""#),
        "plan-name must serialise kebab-case, got:\n{}",
        lines[0]
    );
}

#[test]
fn plan_create_auto_approve_idempotent() {
    // auto-approve Gate-1 contract: running `specrun plan transition <name> approved`
    // after a successful `--auto-approve` create must be a no-op —
    // exit 0, no second `plan.transition.approved` event, plan.yaml
    // unchanged.
    let project = Project::init();

    specrun()
        .current_dir(project.root())
        .args(["plan", "create", "fresh", "--auto-approve"])
        .assert()
        .success();
    let journal = project.root().join(".specify").join("journal.jsonl");
    let before = fs::read_to_string(&journal).expect("read journal.jsonl");
    let before_lines = before.lines().filter(|l| !l.is_empty()).count();
    let plan_before = fs::read_to_string(project.plan_path()).expect("read plan.yaml");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "fresh", "approved"])
        .assert()
        .success();
    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["kind"], "plan");
    assert_eq!(
        body["previous"], "approved",
        "previous lifecycle must already be approved (no-op), got:\n{body}"
    );
    assert_eq!(body["current"], "approved");

    let plan_after = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert_eq!(
        plan_before, plan_after,
        "plan.yaml must not change under the idempotent no-op transition"
    );
    let after = fs::read_to_string(&journal).expect("read journal.jsonl");
    let after_lines = after.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(
        before_lines, after_lines,
        "explicit `transition approved` after --auto-approve must not append a second event"
    );
}

#[test]
fn plan_create_auto_approve_invalid_name() {
    // auto-approve Gate-1 contract: `--auto-approve` does NOT bypass validation. An
    // invalid (non-kebab) name refuses the create with the same
    // exit code and envelope as the post-create path; no `plan.yaml`
    // lands on disk and the journal stays untouched.
    let project = Project::init();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "create", "Bad_Name", "--auto-approve"])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 1, "kebab-case violation surfaces via Error::Diag (exit 1)");
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "change-name-not-kebab");

    assert!(
        !project.plan_path().exists(),
        "plan.yaml must not be written when --auto-approve fails validation"
    );
    let journal = project.root().join(".specify").join("journal.jsonl");
    assert!(
        !journal.exists(),
        "journal must stay empty when --auto-approve validation fails, found: {}",
        journal.display()
    );
}

#[test]
fn create_auto_approve_no_partial_events() {
    // auto-approve Gate-1 contract: validation failure under --auto-approve must not
    // surface a partial-state event sequence — no orphan
    // `plan.amend.authority-override` without the matching
    // `plan.transition.approved`, no half-written plan.yaml. An
    // unknown `--authority-override` slice (the cheapest validation
    // gate to trip on a fresh plan) must refuse the create and
    // leave the journal untouched.
    let project = Project::init();

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "create",
            "fresh",
            "--auto-approve",
            "--authority-override",
            "ghost-slice",
            "criterion=runtime",
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));

    assert!(
        !project.plan_path().exists(),
        "plan.yaml must not be written when --auto-approve + --authority-override fails"
    );
    let journal = project.root().join(".specify").join("journal.jsonl");
    assert!(
        !journal.exists(),
        "journal must stay empty on validation failure, found: {}",
        journal.display()
    );
}

#[test]
fn create_auto_approve_then_validate_passes() {
    // The empty-scaffold + `--auto-approve` combination must still
    // validate cleanly — `--auto-approve` is a Gate-1 consent flag,
    // not a validation bypass, but it also must not introduce any
    // new validation drift on the empty-scaffold path.
    let project = Project::init();

    specrun()
        .current_dir(project.root())
        .args(["plan", "create", "fresh", "--auto-approve"])
        .assert()
        .success();

    let assert =
        specrun().current_dir(project.root()).args(["plan", "validate"]).assert().success();
    assert_eq!(assert.get_output().status.code(), Some(0));
}
