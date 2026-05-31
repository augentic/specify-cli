//! Integration tests for `specrun plan *` — the top-level verb that
//! orchestrates the executable plan at `plan.yaml` (the executable plan contract).
//!
//! These CLI tests stand up a fresh `.specify/` project via `specify
//! init` (mirroring `tests/slice.rs` / `tests/e2e.rs`), seed
//! `plan.yaml` at the repo root by writing YAML directly to disk, and
//! drive the CLI through `assert_cmd`. JSON shapes are pinned by
//! checked-in fixtures under `tests/fixtures/plan/`; regenerate them
//! with
//! `REGENERATE_GOLDENS=1 cargo test --test plan_orchestrate`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use serde_json::Value;
use specify_workflow::change::Plan;
use tempfile::{TempDir, tempdir};

mod common;
use common::{
    Project, assert_golden_at, init_hub, omnia_schema_dir, parse_stderr, parse_stdout, repo_root,
    specrun,
};

fn plan_fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/plan")
}

fn assert_golden(name: &str, actual: Value) {
    assert_golden_at(&plan_fixtures(), name, actual);
}

// -- test seeds --------------------------------------------------------

const CLEAN_PLAN: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: pending
  - name: b
    project: default
    status: pending
    depends-on: [a]
";

const DUPLICATE_NAME_PLAN: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: pending
  - name: foo
    project: default
    status: pending
";

const A_DONE_B_PENDING: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: pending
";

const A_IN_PROGRESS: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: in-progress
";

const ALL_DONE: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: done
";

/// All entries done — `next` reports `drained` post-2.0 (the
/// previous "stuck" semantics relied on the now-removed `failed`
/// state). Kept under the historical name for fixture continuity;
/// the test asserts the new `drained` reason.
const STUCK_PLAN: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: done
    depends-on: [a]
";

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
        .args(["--format", "json", "plan", "add", "foo", "--target", "contracts@v1"])
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

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "contracts@v1"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "contracts@v1"])
        .assert()
        .failure();
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

    let assert = specrun()
        .current_dir(project.root())
        .args(["plan", "add", "NotKebab", "--target", "contracts@v1"])
        .assert()
        .failure();
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
    target: contracts@v1
    status: pending
  - name: b
    target: contracts@v1
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
    target: contracts@v1
    status: pending
  - name: b
    target: contracts@v1
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
            "--target",
            "contracts@v1",
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

// -- plan archive (L1.K) ----------------------------------------------

fn today_yyyymmdd() -> String {
    jiff::Timestamp::now().strftime("%Y%m%d").to_string()
}

/// Replace any `-YYYYMMDD` date stamp in JSON strings with a stable
/// placeholder so the archive-success golden is date-insensitive.
fn strip_date_stamps(value: &mut Value) {
    fn visit(re: &regex::Regex, v: &mut Value) {
        match v {
            Value::String(s) if re.is_match(s) => {
                *s = re.replace_all(s, "-<YYYYMMDD>").into_owned();
            }
            Value::Array(items) => {
                for item in items {
                    visit(re, item);
                }
            }
            Value::Object(map) => {
                for (_k, v) in map.iter_mut() {
                    visit(re, v);
                }
            }
            _ => {}
        }
    }
    let re = regex::Regex::new(r"-\d{8}\b").expect("regex compiles");
    visit(&re, value);
}

fn archive_dir(project: &Project) -> PathBuf {
    project.root().join(".specify/archive/plans")
}

#[test]
fn plan_archive_happy_path_text() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);

    let assert = specrun().current_dir(project.root()).args(["plan", "archive"]).assert().success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert!(
        stdout.contains("Archived plan to"),
        "stdout should announce archive path, got: {stdout:?}"
    );

    assert!(!project.plan_path().exists(), "original plan.yaml must be gone");
    let archived = archive_dir(&project).join(format!("demo-{}.yaml", today_yyyymmdd()));
    assert!(archived.exists(), "archived file not found at {}", archived.display());
}

#[test]
fn plan_archive_happy_path_json() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "archive"])
        .assert()
        .success();
    let mut actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["plan"]["name"], "demo");
    assert!(
        actual["archived"].as_str().unwrap_or_default().contains("demo-"),
        "archived path should contain the plan name, got: {}",
        actual["archived"]
    );

    strip_date_stamps(&mut actual);
    assert_golden("archive-success.json", actual);
}

#[test]
fn plan_archive_refuses_without_force() {
    let project = Project::init();
    project.seed_plan(A_DONE_B_PENDING);

    let assert = specrun().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.contains('b'),
        "stderr should mention the pending entry name 'b', got: {stderr:?}"
    );
    assert!(stderr.contains("--force"), "stderr should suggest --force, got: {stderr:?}");

    assert!(project.plan_path().exists(), "plan.yaml must still exist");
    assert!(
        !archive_dir(&project).exists()
            || !archive_dir(&project).join(format!("demo-{}.yaml", today_yyyymmdd())).exists(),
        "no archive file should be written on refusal"
    );
}

#[test]
fn plan_archive_refuses_json_lists_entries() {
    let project = Project::init();
    project.seed_plan(A_DONE_B_PENDING);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "archive"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));

    // The typed failure envelope is written to stderr.
    let actual = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(actual["error"], "plan-has-outstanding-work");
    assert_eq!(actual["exit-code"], 1);
    let message = actual["message"].as_str().expect("message string");
    assert!(message.contains('b'), "message should mention the pending entry 'b': {message}");

    assert_golden("archive-outstanding-work.json", actual);
}

#[test]
fn plan_archive_with_force_succeeds() {
    let project = Project::init();
    project.seed_plan(A_DONE_B_PENDING);

    specrun().current_dir(project.root()).args(["plan", "archive", "--force"]).assert().success();

    let archived = archive_dir(&project).join(format!("demo-{}.yaml", today_yyyymmdd()));
    assert!(archived.exists(), "archived file missing at {}", archived.display());
    let contents = fs::read_to_string(&archived).expect("read archived yaml");
    assert!(
        contents.contains("name: b"),
        "archived yaml should preserve pending entry 'b':\n{contents}"
    );
    assert!(
        contents.contains("status: pending"),
        "archived yaml should preserve pending status verbatim:\n{contents}"
    );
}

#[test]
fn archive_filename_kebab_plus_date() {
    let project = Project::init();
    project.seed_plan(
        "\
name: my-change
slices: []
",
    );

    specrun().current_dir(project.root()).args(["plan", "archive"]).assert().success();

    let re = regex::Regex::new(r"^my-change-\d{8}\.yaml$").expect("regex compiles");
    let entries: Vec<String> = fs::read_dir(archive_dir(&project))
        .expect("read archive dir")
        .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one archive file, got: {entries:?}");
    assert!(
        re.is_match(&entries[0]),
        "archive filename {} should match `my-change-<YYYYMMDD>.yaml`",
        entries[0]
    );
}

#[test]
fn plan_archive_refuses_when_dest_exists() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);

    let dest_dir = archive_dir(&project);
    fs::create_dir_all(&dest_dir).expect("mkdir archive dir");
    let dest = dest_dir.join(format!("demo-{}.yaml", today_yyyymmdd()));
    fs::write(&dest, "prior: content\n").expect("seed prior archive");

    let assert = specrun().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("already exists"),
        "stderr should mention 'already exists', got: {stderr:?}"
    );

    assert!(project.plan_path().exists(), "original plan.yaml must be untouched");
    let dest_contents = fs::read_to_string(&dest).expect("read prior archive");
    assert_eq!(
        dest_contents, "prior: content\n",
        "pre-existing archive destination must not be overwritten"
    );
}

#[test]
fn plan_archive_missing_file_errors() {
    let project = Project::init();
    // Deliberately do NOT seed plan.yaml.

    let assert = specrun().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("plan.yaml not found at"),
        "stderr should mention 'plan.yaml not found at', got: {stderr:?}"
    );
}

// -- plan archive co-move of working directory (L3.B) ---------------

/// Seed `.specify/plans/<name>/` with the given files and return
/// the directory path.
fn seed_working_dir(project: &Project, plan_name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = project.root().join(".specify/plans").join(plan_name);
    fs::create_dir_all(&dir).expect("mkdir plans working dir");
    for (name, bytes) in files {
        fs::write(dir.join(name), bytes).expect("seed working file");
    }
    dir
}

#[test]
fn plan_archive_co_moves_working_dir() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);
    let working_dir = seed_working_dir(
        &project,
        "demo",
        &[("discovery.md", b"# discovery\n"), ("proposal.md", b"# proposal\n")],
    );

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "archive"])
        .assert()
        .success();
    let mut actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["plan"]["name"], "demo");
    assert!(
        actual["archived"].as_str().unwrap_or_default().contains("demo-"),
        "archived path should contain the plan name"
    );
    assert!(
        actual["archived-plans-dir"].as_str().unwrap_or_default().contains("demo-"),
        "archived-plans-dir should contain the plan name, got: {}",
        actual["archived-plans-dir"]
    );

    assert!(!working_dir.exists(), ".specify/plans/demo/ must be gone after archive");
    let archived_dir = archive_dir(&project).join(format!("demo-{}", today_yyyymmdd()));
    assert!(archived_dir.is_dir(), "co-moved dir missing at {}", archived_dir.display());
    assert_eq!(
        fs::read_to_string(archived_dir.join("discovery.md")).expect("read"),
        "# discovery\n"
    );
    assert_eq!(fs::read_to_string(archived_dir.join("proposal.md")).expect("read"), "# proposal\n");

    strip_date_stamps(&mut actual);
    assert_golden("archive-success-with-working-dir.json", actual);
}

#[test]
fn plan_archive_no_working_dir_json() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "archive"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(
        actual["archived-plans-dir"],
        Value::Null,
        "no working dir must surface archived-plans-dir: null, got: {}",
        actual["archived-plans-dir"]
    );
}

#[test]
fn plan_archive_co_move_collision_halts() {
    let project = Project::init();
    project.seed_plan(ALL_DONE);
    let working_dir = seed_working_dir(&project, "demo", &[("notes.md", b"# notes\n")]);

    // Pre-create the co-move destination only; the plan.yaml
    // archive destination is clear, so this hits the working-dir
    // preflight specifically.
    let dest_dir = archive_dir(&project).join(format!("demo-{}", today_yyyymmdd()));
    fs::create_dir_all(&dest_dir).expect("seed collision dir");

    let assert = specrun().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("already exists"),
        "stderr should name 'already exists', got: {stderr:?}"
    );

    // Preflight contract: plan.yaml must be untouched on collision.
    assert!(
        project.plan_path().exists(),
        "plan.yaml MUST be untouched when working-dir preflight fails"
    );
    assert!(working_dir.is_dir(), "source working dir must be untouched on collision");
    let plan_archive = archive_dir(&project).join(format!("demo-{}.yaml", today_yyyymmdd()));
    assert!(!plan_archive.exists(), "plan.yaml must not have been archived on collision");
    assert!(
        dest_dir.is_dir() && fs::read_dir(&dest_dir).expect("read").next().is_none(),
        "pre-existing collision dir must remain empty"
    );
}

/// `specrun plan validate` surfaces a malformed `registry.yaml`
/// alongside plan validation results — the shape-validation hook
/// complementing the dedicated `specrun registry validate`
/// verb.
#[test]
fn plan_validate_surfaces_registry_errors() {
    let project = Project::init();
    // Seed a minimal, structurally-valid plan so `change plan validate`
    // doesn't exit on the plan load itself.
    project.seed_plan("name: demo\nslices: []\n");
    // Then stomp the registry with an illegal version.
    fs::write(project.root().join("registry.yaml"), "version: 2\nprojects: []\n")
        .expect("write bad registry");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "validate"])
        .assert()
        .failure();
    let value = parse_stdout(&assert.get_output().stdout, project.root());
    let results = value["results"].as_array().expect("results array");
    let registry_findings: Vec<&Value> =
        results.iter().filter(|r| r["code"] == "registry-shape").collect();
    assert_eq!(
        registry_findings.len(),
        1,
        "expected one registry-shape finding, got: {results:#?}"
    );
    assert_eq!(registry_findings[0]["severity"], "error");
    let msg = registry_findings[0]["message"].as_str().expect("message string");
    assert!(msg.contains("version"), "expected version in message, got: {msg}");
    assert_eq!(value["passed"], false);
}

// ---- planning-path workspace smoke — planning-path smoke (Stage A/B, manifest, Layer 2) ----

#[test]
fn planning_stage_ab_brief_and_validate() {
    let project = Project::init();
    specrun()
        .current_dir(project.root())
        .args(["plan", "create", "planning-path", "--source", "app=code-typescript:."])
        .assert()
        .success();
    specrun().current_dir(project.root()).args(["plan", "validate"]).assert().success();
}

// ---- specrun plan validate health diagnostics (plan validate health diagnostics) ----
//
// `plan validate` carries the three surviving health diagnostics
// (`cycle-in-depends-on`, `orphan-source`,
// `stale-workspace-clone`) alongside its base shape rules. The
// `unreachable-entry` diagnostic retired in source/target adapter split alongside the
// per-entry `failed`/`skipped` states it relied on.

fn init_omnia_project(tmp: &TempDir) {
    specrun()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();
}

#[test]
fn validate_reports_all_health_diagnostics() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    // Authoring a plan that intentionally exercises all four doctor
    // checks at once. We hand-write `plan.yaml` because the CLI's own
    // `plan create` path enforces validation at write time and would
    // refuse the cycle / unknown-source cases below.
    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
             sources:\n\
             \x20\x20monolith:\n\
             \x20\x20\x20\x20adapter: code-typescript\n\
             \x20\x20\x20\x20path: /tmp/legacy\n\
             \x20\x20orphaned:\n\
             \x20\x20\x20\x20adapter: code-typescript\n\
             \x20\x20\x20\x20path: /tmp/elsewhere\n\
             slices:\n\
             \x20\x20- name: cyclic-a\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyclic-b]\n\
             \x20\x20- name: cyclic-b\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyclic-a]\n\
             \x20\x20- name: orphaned-source-user\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20sources: [monolith]\n",
    )
    .unwrap();

    // Hand-write a registry at the repo root, so we can exercise
    // stale-clone with a deterministic fixture: a clone slot whose
    // origin remote disagrees with the registry.
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
             projects:\n\
             \x20\x20- name: alpha\n\
             \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
             \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();
    let slot = tmp.path().join(".specify/workspace/alpha");
    fs::create_dir_all(&slot).unwrap();
    let init = ProcessCommand::new("git").arg("-C").arg(&slot).arg("init").output().unwrap();
    assert!(init.status.success(), "git init failed: {}", String::from_utf8_lossy(&init.stderr));
    let remote = ProcessCommand::new("git")
        .arg("-C")
        .arg(&slot)
        .args(["remote", "add", "origin", "git@github.com:old/alpha.git"])
        .output()
        .unwrap();
    assert!(
        remote.status.success(),
        "git remote add failed: {}",
        String::from_utf8_lossy(&remote.stderr)
    );

    let assert =
        specrun().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8");
    let value: Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    let results = value["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "validate with broken plan must surface results: {value}");
    let codes: Vec<&str> = results.iter().filter_map(|r| r["code"].as_str()).collect();

    for expected in ["cycle-in-depends-on", "orphan-source", "stale-workspace-clone"] {
        assert!(
            codes.contains(&expected),
            "validate must emit `{expected}` for the synthetic fixture; saw: {codes:?}"
        );
    }

    // Exit code must be ValidationFailed (2) because the cycle is
    // error-severity.
    let code = output.status.code().expect("exit code");
    assert_eq!(code, 2, "error-severity diagnostics must yield exit 2, got {code}");
}

#[test]
fn validate_reports_adapter_mismatch() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
             slices:\n\
             \x20\x20- name: alpha-slice\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20project: alpha\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
             projects:\n\
             \x20\x20- name: alpha\n\
             \x20\x20\x20\x20url: git@github.com:org/alpha.git\n\
             \x20\x20\x20\x20adapter: omnia@v1\n",
    )
    .unwrap();

    let slot_specify = tmp.path().join(".specify/workspace/alpha/.specify");
    fs::create_dir_all(&slot_specify).unwrap();
    fs::write(slot_specify.join("project.yaml"), "name: alpha\nadapter: vectis@v1\n").unwrap();

    let assert =
        specrun().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
    let value: Value =
        serde_json::from_str(&String::from_utf8(assert.get_output().stdout.clone()).expect("utf8"))
            .expect("stdout is JSON");
    let results = value["results"].as_array().expect("results array");
    let mismatch: Vec<&Value> =
        results.iter().filter(|r| r["code"] == "adapter-mismatch-workspace").collect();
    assert_eq!(
        mismatch.len(),
        1,
        "expected one adapter-mismatch-workspace finding, got: {results:#?}"
    );
    assert_eq!(mismatch[0]["severity"], "warning");
    let msg = mismatch[0]["message"].as_str().expect("message string");
    assert!(msg.contains("alpha"), "expected clone name in message, got: {msg}");
    assert!(msg.contains("vectis@v1"), "expected slot adapter in message, got: {msg}");
    assert!(msg.contains("omnia@v1"), "expected registry adapter in message, got: {msg}");
    assert_eq!(value["passed"], true, "adapter mismatch is warning-only");
}

#[test]
fn plan_validate_payloads_round_trip_typed() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    // Minimal plan that exercises just the cycle and orphan-source
    // checks — enough to confirm the typed payload deserialises
    // cleanly.
    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
             sources:\n\
             \x20\x20orphan-key:\n\
             \x20\x20\x20\x20adapter: code-typescript\n\
             \x20\x20\x20\x20path: /tmp/somewhere\n\
             slices:\n\
             \x20\x20- name: cyc-a\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyc-b]\n\
             \x20\x20- name: cyc-b\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyc-a]\n",
    )
    .unwrap();

    let assert =
        specrun().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    let results = value["results"].as_array().expect("results array");

    let cycle = results
        .iter()
        .find(|d| d["code"] == "cycle-in-depends-on")
        .expect("expected cycle-in-depends-on diagnostic");
    let cycle_path = cycle["data"]["cycle"].as_array().expect("cycle path is array");
    let names: Vec<String> =
        cycle_path.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    assert_eq!(
        names,
        vec!["cyc-a".to_string(), "cyc-b".to_string(), "cyc-a".to_string()],
        "cycle path must close on the first node"
    );
    assert_eq!(cycle["data"]["kind"], "cycle");

    let orphan = results
        .iter()
        .find(|d| d["code"] == "orphan-source")
        .expect("expected orphan-source diagnostic");
    assert_eq!(orphan["data"]["kind"], "orphan-source");
    assert_eq!(orphan["data"]["key"], "orphan-key");
    assert_eq!(orphan["severity"], "warning");
}

#[test]
fn plan_validate_healthy_exits_zero() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "plan", "create", "demo"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "plan", "validate"])
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(
        value["results"].as_array().unwrap().len(),
        0,
        "empty plan must emit zero results: {value}"
    );
    assert_eq!(value["passed"], true, "empty plan must pass: {value}");
}

// ---- Wave 1.1 — per-slice source binding flag reshape ----
//
// The reshape replaces 1.x's bare `--sources <key>` repeater with the
// `<key>=<lead>` wire form, accepting the bare `<key>`
// shorthand only as sugar for `{ source, lead: <slice.name> }`
// per workflow §`Slice.sources`.

const W11_PLAN: &str = "\
name: w11
sources:
  intent:
    adapter: intent
    value: \"Demo intent value.\"
  identity-design-notes:
    adapter: documentation
    path: ./docs
slices: []
";

#[test]
fn plan_add_structured_sources_round_trips() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "add",
            "foo",
            "--target",
            "omnia@v1",
            "--sources",
            "identity-design-notes=user-registration",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("source: identity-design-notes")
            && saved.contains("lead: user-registration"),
        "structured form must round-trip to disk:\n{saved}"
    );
}

#[test]
fn plan_add_bare_source_round_trips() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    // Slice name `add-search-filter`; bare `--sources intent` is
    // sugar for `{ source: intent, lead: add-search-filter }`.
    specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "add",
            "add-search-filter",
            "--target",
            "omnia@v1",
            "--sources",
            "intent",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    // Bare form must appear on disk as the YAML scalar `intent`,
    // not the structured `{ source, lead }` mapping.
    assert!(
        saved.contains("  - intent"),
        "bare shorthand must round-trip to the unquoted scalar form:\n{saved}"
    );
    assert!(
        !saved.contains("lead: add-search-filter"),
        "lead=slice.name must collapse to bare form:\n{saved}"
    );
}

#[test]
fn plan_add_structured_lead_differs() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "add",
            "foo",
            "--target",
            "omnia@v1",
            "--sources",
            "intent=different-candidate",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("lead: different-candidate"),
        "structured form must stay structured when lead != slice.name:\n{saved}"
    );
}

#[test]
fn add_rejects_dangling_equals() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "add",
            "foo",
            "--target",
            "omnia@v1",
            "--sources",
            "intent=",
        ])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "malformed --sources must exit 2 (argument error), got {code}");
}

#[test]
fn plan_amend_add_source_appends_binding() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "omnia@v1", "--sources", "intent"])
        .assert()
        .success();

    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--add-source", "identity-design-notes=user-registration"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("source: identity-design-notes"),
        "amend --add-source must append the binding:\n{saved}"
    );
}

#[test]
fn plan_amend_remove_source_drops_binding() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "foo",
            "--target",
            "omnia@v1",
            "--sources",
            "intent",
            "--sources",
            "identity-design-notes=foo",
        ])
        .assert()
        .success();

    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--remove-source", "intent"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(!saved.contains("- intent"), "amend --remove-source must drop the binding:\n{saved}");
    assert!(saved.contains("identity-design-notes"), "non-targeted bindings must remain:\n{saved}");
}

#[test]
fn amend_remove_source_unknown_key_errors() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "omnia@v1", "--sources", "intent"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "amend", "foo", "--remove-source", "no-such-key"])
        .assert()
        .failure();
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "plan-binding-not-found");
}

#[test]
fn amend_divergence_accepted_writes() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "omnia@v1"])
        .assert()
        .success();

    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--divergence", "accepted"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("divergence: accepted"),
        "amend --divergence accepted must write the field:\n{saved}"
    );
}

#[test]
fn amend_divergence_rejected_writes() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "omnia@v1"])
        .assert()
        .success();

    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--divergence", "rejected"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("divergence: rejected"),
        "amend --divergence rejected must write the field:\n{saved}"
    );
}

#[test]
fn amend_divergence_likely_writes() {
    // divergence and writer-ownership contract: `--divergence likely` is operator-settable from
    // the CLI; the field is byte-identical to the legacy
    // skill-written `divergence: likely` line.
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "omnia@v1"])
        .assert()
        .success();

    specrun()
        .current_dir(project.root())
        .args(["plan", "amend", "foo", "--divergence", "likely"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("divergence: likely"),
        "amend --divergence likely must write the field:\n{saved}"
    );
}

#[test]
fn plan_amend_divergence_none_refused() {
    let project = Project::init();
    project.seed_plan(W11_PLAN);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--target", "omnia@v1"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "amend", "foo", "--divergence", "none"])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "implicit --divergence none must exit 2 (argument error)");
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "argument");
}

// -- plan {create,add,amend} --authority-override (per-slice authority override) --------

const AUTHORITY_OVERRIDE_PLAN: &str = "\
name: identity-revamp
sources:
  legacy:
    adapter: code-typescript
    path: ./legacy-monolith
  runtime:
    adapter: captures
    path: ./captures/replays
slices:
  - name: identity-user-registration
    project: default
    target: omnia@v1
    status: pending
    sources:
      - source: legacy
        lead: user-registration
      - source: runtime
        lead: user-registration
";

fn read_journal_lines(project: &Project) -> Vec<String> {
    let path = project.root().join(".specify").join("journal.jsonl");
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(&path)
        .expect("read journal")
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn amend_authority_override_round_trips() {
    // per-slice authority override happy path: set an override via `amend`, re-read
    // `plan.yaml` and confirm the field landed under the named
    // slice; `slice validate` accepts it because `runtime` is in
    // the slice's `sources[]`.
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "identity-user-registration",
            "requirement=runtime",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("authority-override:"),
        "plan.yaml must contain authority-override block, got:\n{saved}"
    );
    assert!(
        saved.contains("requirement: runtime"),
        "plan.yaml must record requirement: runtime, got:\n{saved}"
    );

    // Plan-level validate passes — orphan check only fires for bad keys.
    specrun().current_dir(project.root()).args(["plan", "validate"]).assert().success();

    // Journal carries exactly one PlanAmendAuthorityOverride event.
    let lines = read_journal_lines(&project);
    assert_eq!(lines.len(), 1, "expected one journal event, got:\n{lines:?}");
    let line = &lines[0];
    assert!(line.contains(r#""event":"plan.amend.authority-override""#));
    assert!(line.contains(r#""action":"set""#));
    assert!(line.contains(r#""claim-kind":"requirement""#));
    assert!(line.contains(r#""source":"runtime""#));
    assert!(line.contains(r#""slice-name":"identity-user-registration""#));
}

#[test]
fn plan_amend_override_orphan_refused() {
    // per-slice authority override gate: refuse the `specrun plan amend` write when
    // the authority-override value names a source key not present
    // in the slice's `sources[]` list (`phantom`). The orphan
    // check runs in `Plan::validate` (folded in by Change 2.3),
    // which `mutate_authority_overrides` re-runs after the
    // override mutations to catch the case where a brand-new
    // entry would introduce drift.
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);
    let before = fs::read_to_string(project.plan_path()).expect("read plan");

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "identity-user-registration",
            "requirement=phantom",
        ])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "orphan source must exit 2 (validation_failed)");
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "slice-authority-override-orphan-source");

    let after = fs::read_to_string(project.plan_path()).expect("read plan");
    assert_eq!(before, after, "plan.yaml must not change on the refused write");
    assert!(
        read_journal_lines(&project).is_empty(),
        "journal must stay empty on the refused write"
    );
}

#[test]
fn slice_validate_authority_override_orphan() {
    // per-slice authority override — `specrun slice validate` is the per-slice gate
    // that mirrors the plan-level check; it runs before refine
    // synthesises any artifacts so a bad override is caught
    // before downstream writes. Hand-edit `plan.yaml` to seed an
    // orphan entry (the only legal path is via the CLI, which
    // refuses, so we splice the file to exercise the gate without
    // bypassing the JSON-schema enforcement).
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);
    let original = fs::read_to_string(project.plan_path()).expect("read plan");
    // Splice the orphan override into the first slice. Anchor on
    // the `status: pending` line so the YAML structure stays
    // wellformed regardless of source-binding ordering.
    let needle = "    status: pending\n    sources:";
    let replacement =
        "    status: pending\n    authority-override:\n      requirement: phantom\n    sources:";
    let patched = original.replacen(needle, replacement, 1);
    assert_ne!(patched, original, "splice precondition: needle present in plan.yaml");
    fs::write(project.plan_path(), patched.as_bytes()).expect("write patched plan");

    // Create the slice dir so `slice validate` runs to the gate
    // (other artifacts absent → no spec/evidence findings).
    let slices_dir =
        project.root().join(".specify").join("slices").join("identity-user-registration");
    fs::create_dir_all(&slices_dir).expect("mkdir slice");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "identity-user-registration"])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "slice validate orphan must exit 2 (validation_failed)");
    // `slice validate` renders the DiagnosticReport on stdout and fails
    // payload-free on stderr; the orphan finding lives on the report.
    let report = parse_stdout(&assert.get_output().stdout, project.root());
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|r| r["rule-id"] == "slice-authority-override-orphan-source"),
        "expected orphan finding from slice validate: {findings:#?}"
    );
}

#[test]
fn amend_clear_authority_override_removes_one() {
    // per-slice authority override: `--clear-authority-override <slice> <kind>` peels
    // off a single entry; the rest of the map survives. Journal
    // records the Clear without any spurious Set events for the
    // surviving entries.
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "identity-user-registration",
            "requirement=runtime",
            "--authority-override",
            "identity-user-registration",
            "criterion=legacy",
        ])
        .assert()
        .success();

    // Wipe the journal so we observe only the second amend's events.
    fs::write(project.root().join(".specify").join("journal.jsonl"), "").expect("clear journal");

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--clear-authority-override",
            "identity-user-registration",
            "requirement",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        !saved.contains("requirement: runtime"),
        "requirement entry must be cleared, got:\n{saved}"
    );
    assert!(
        saved.contains("criterion: legacy"),
        "criterion entry must survive the targeted clear, got:\n{saved}"
    );

    let lines = read_journal_lines(&project);
    assert_eq!(lines.len(), 1, "expected one Clear event, got:\n{lines:?}");
    let line = &lines[0];
    assert!(line.contains(r#""action":"clear""#));
    assert!(line.contains(r#""claim-kind":"requirement""#));
}

#[test]
fn plan_amend_clear_overrides_wipes_map() {
    // per-slice authority override: `--clear-authority-overrides <slice>` wipes the
    // entire `authority-override` map for that slice and emits one
    // Clear event per kind that was present before the wipe.
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "identity-user-registration",
            "requirement=runtime",
            "--authority-override",
            "identity-user-registration",
            "criterion=legacy",
        ])
        .assert()
        .success();
    fs::write(project.root().join(".specify").join("journal.jsonl"), "").expect("clear journal");

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--clear-authority-overrides",
            "identity-user-registration",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        !saved.contains("authority-override:"),
        "authority-override map must elide once empty, got:\n{saved}"
    );

    let lines = read_journal_lines(&project);
    assert_eq!(lines.len(), 2, "expected two per-kind Clear events, got:\n{lines:?}");
    let combined = lines.join("\n");
    assert!(combined.contains(r#""claim-kind":"requirement""#));
    assert!(combined.contains(r#""claim-kind":"criterion""#));
    assert!(
        lines.iter().all(|l| l.contains(r#""action":"clear""#)),
        "every emitted event must carry action: clear, got:\n{combined}"
    );
}

#[test]
fn amend_authority_override_set_then_clear() {
    // per-slice authority override deterministic-order rule: a same-invocation set +
    // clear pair on the same `(slice, kind)` resolves to the
    // cleared state; the journal records the Clear (not the Set).
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "identity-user-registration",
            "requirement=runtime",
            "--clear-authority-override",
            "identity-user-registration",
            "requirement",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        !saved.contains("requirement: runtime"),
        "set+clear on same kind must resolve to cleared, got:\n{saved}"
    );
    let lines = read_journal_lines(&project);
    assert_eq!(lines.len(), 1, "expected one Clear event (set was elided), got:\n{lines:?}");
    assert!(
        lines[0].contains(r#""action":"clear""#),
        "the surviving event must be a clear, got:\n{}",
        lines[0]
    );
}

#[test]
fn add_authority_override_seeds_map() {
    // per-slice authority override add path: `plan add --authority-override
    // <kind>=<key>` pre-seeds the override map at create time. Each
    // entry fires one PlanAmendAuthorityOverride / `set` event.
    let project = Project::init();
    project.seed_plan(
        "name: identity-revamp\n\
        sources:\n\
        \x20\x20legacy:\n\
        \x20\x20\x20\x20adapter: code-typescript\n\
        \x20\x20\x20\x20path: ./legacy\n\
        \x20\x20runtime:\n\
        \x20\x20\x20\x20adapter: captures\n\
        \x20\x20\x20\x20path: ./captures/replays\n\
        slices: []\n",
    );

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "identity-user-registration",
            "--target",
            "omnia@v1",
            "--sources",
            "legacy=user-registration",
            "--sources",
            "runtime=user-registration",
            "--authority-override",
            "requirement=runtime",
            "--authority-override",
            "criterion=legacy",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(saved.contains("authority-override:"));
    assert!(saved.contains("requirement: runtime"));
    assert!(saved.contains("criterion: legacy"));

    let lines = read_journal_lines(&project);
    assert_eq!(lines.len(), 2, "one event per seeded kind, got:\n{lines:?}");
    for line in &lines {
        assert!(line.contains(r#""action":"set""#));
        assert!(line.contains(r#""slice-name":"identity-user-registration""#));
    }
}

#[test]
fn amend_authority_override_unknown_slice_refused() {
    // per-slice authority override: unknown `--authority-override <slice>` must
    // refuse at exit 2 before any plan.yaml write happens.
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);
    let before = fs::read_to_string(project.plan_path()).expect("read plan");

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "ghost-slice",
            "requirement=runtime",
        ])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "unknown slice must exit 2 (validation_failed)");

    let after = fs::read_to_string(project.plan_path()).expect("read plan");
    assert_eq!(before, after, "plan.yaml must be unchanged on refusal");
    assert!(read_journal_lines(&project).is_empty(), "no journal events on the refused write");
}

#[test]
fn plan_amend_override_bad_kind_refused() {
    // per-slice authority override: `<kind>` is validated against the closed
    // ClaimKind enum at the CLI boundary — clap surfaces a usage
    // diagnostic (exit 2) before any plan mutation runs.
    let project = Project::init();
    project.seed_plan(AUTHORITY_OVERRIDE_PLAN);

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "identity-user-registration",
            "--authority-override",
            "identity-user-registration",
            "bogus-kind=runtime",
        ])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "invalid kind must exit 2");
    // The kind enum is enforced inside our argument parser (not by
    // clap's value_parser), so the error surfaces as a plain
    // `Error::Argument` whose stderr is human text rather than
    // JSON. We assert the exit code and the human message body.
    let stderr_str = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr_str.contains("bogus-kind"),
        "expected the bad kind name to appear in stderr, got:\n{stderr_str}"
    );
}

// ===================================================================
// `specrun plan propose` — RFC-29 D2 lead reconciliation
// (end-to-end coverage of the shipped command surface).
//
// `--dry-run` emits the `kind: request` envelope (flat lead catalog +
// project topology) and writes nothing; `--from` schema-gates the
// agent response, projects it onto `plan.yaml.slices[]`, and emits the
// paired `plan.reconcile.{agent,completed}` journal events. JSON shapes
// are pinned by goldens under `tests/fixtures/plan/`; regenerate with
// `REGENERATE_GOLDENS=1 cargo test --test plan_orchestrate`.
// ===================================================================

// -- propose seeds ----------------------------------------------------

/// N=1 plan: a single `intent` source, no slices yet (replaceable).
const PROPOSE_PLAN_N1: &str = "\
name: demo
sources:
  intent:
    adapter: intent
    value: \"Fix a typo in user.rs.\"
slices: []
";

/// N=1 surveyed inventory: one `intent` lead.
const PROPOSE_DISCOVERY_N1: &str = "\
## Lead inventory

### intent:fix-typo

- lead: fix-typo
- source: intent
- synopsis: Fix a typo in user.rs.
";

/// N=1 agent response: omits `project` (kernel auto-binds the sole
/// project) and `name` (kernel derives it from `scope`).
const PROPOSE_RESPONSE_N1: &str = r#"{
  "version": 1,
  "kind": "response",
  "slices": [
    { "scope": "fix-typo", "sources": [{ "source": "intent", "lead": "fix-typo" }] }
  ]
}"#;

/// Hub registry with two projects bound to different target adapters —
/// the topology the fan-out response binds against.
const PROPOSE_REGISTRY_HUB: &str = "\
version: 1
projects:
  - name: identity-contracts
    url: git@github.com:org/identity-contracts.git
    adapter: contracts@v1
    description: Versioned API contracts crate for the identity domain.
  - name: identity-service
    url: git@github.com:org/identity-service.git
    adapter: omnia@v1
    description: Omnia identity service implementing auth and password flows.
";

/// Hub surveyed inventory: four leads across `docs` + `legacy` (the
/// proposal-schema envelope example, in document order).
const PROPOSE_DISCOVERY_HUB: &str = "\
## Lead inventory

### docs:identity-api

- lead: identity-api
- source: docs
- synopsis: Identity API contract for authentication and account access.

### legacy:identity-api

- lead: identity-api
- source: legacy
- synopsis: Legacy identity endpoints.

### docs:password-reset

- lead: password-reset
- source: docs
- synopsis: Users can request a password reset email.

### legacy:reset-password

- lead: reset-password
- source: legacy
- synopsis: Legacy reset-password flow.
";

/// Hub plan declaring the two surveyed source keys, no slices yet.
const PROPOSE_PLAN_HUB: &str = "\
name: identity-revamp
sources:
  docs:
    adapter: documentation
    path: ./docs
  legacy:
    adapter: code-typescript
    path: ./legacy
slices: []
";

/// Multi-source fan-out response (the proposal-schema envelope
/// example): `identity-api` fans out to two projects (shared `scope`,
/// identical `sources`); `password-reset` is a 1:1 scope matched across
/// sources by summary, with a `depends-on` edge into the contracts
/// crate.
const PROPOSE_RESPONSE_FANOUT: &str = r#"{
  "version": 1,
  "kind": "response",
  "slices": [
    {
      "name": "identity-contracts",
      "scope": "identity-api",
      "sources": [
        { "source": "docs", "lead": "identity-api" },
        { "source": "legacy", "lead": "identity-api" }
      ],
      "project": "identity-contracts",
      "rationale": "identity API surface matched by shared slug across docs + legacy"
    },
    {
      "name": "identity-service",
      "scope": "identity-api",
      "sources": [
        { "source": "docs", "lead": "identity-api" },
        { "source": "legacy", "lead": "identity-api" }
      ],
      "project": "identity-service",
      "depends-on": ["identity-contracts"]
    },
    {
      "name": "password-reset",
      "scope": "password-reset",
      "sources": [
        { "source": "docs", "lead": "password-reset" },
        { "source": "legacy", "lead": "reset-password" }
      ],
      "project": "identity-service",
      "rationale": "password-reset (docs) and reset-password (legacy) are the same flow by synopsis judgment"
    }
  ]
}"#;

// -- propose helpers --------------------------------------------------

/// Build a minimal `discovery.md` body with one `### source:lead` block
/// per `(source, lead)` pair — mirrors the kernel unit-test
/// seeding so negative fixtures stay one-liners.
fn discovery_doc(leads: &[(&str, &str)]) -> String {
    use std::fmt::Write as _;
    let mut body = String::from("## Lead inventory\n\n");
    for (source, lead) in leads {
        let _ = write!(
            body,
            "### {source}:{lead}\n\n\
             - lead: {lead}\n\
             - source: {source}\n\
             - synopsis: {lead} synopsis.\n\n",
        );
    }
    body
}

fn seed_discovery(root: &Path, body: &str) {
    fs::write(root.join("discovery.md"), body).expect("write discovery.md");
}

/// Write a `--from` response file under `root`, returning its path.
fn write_response(root: &Path, body: &str) -> PathBuf {
    let path = root.join("response.json");
    fs::write(&path, body).expect("write response.json");
    path
}

/// Scaffold a hub-mode project in a fresh tempdir, seeding
/// `registry.yaml`, `discovery.md`, and `plan.yaml`.
fn hub_project(registry: &str, discovery: &str, plan: &str) -> TempDir {
    let tmp = tempdir().expect("tempdir");
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("registry.yaml"), registry).expect("write registry.yaml");
    seed_discovery(tmp.path(), discovery);
    fs::write(tmp.path().join("plan.yaml"), plan).expect("write plan.yaml");
    tmp
}

/// Run `plan propose --from <body>` expecting an exit-2 abort and
/// return the parsed `--format json` stderr envelope.
fn propose_from_stderr(root: &Path, body: &str) -> Value {
    let response = write_response(root, body);
    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "plan", "propose", "--from"])
        .arg(&response)
        .assert()
        .failure();
    assert_eq!(
        assert.get_output().status.code(),
        Some(2),
        "every propose --from invariant aborts at exit 2"
    );
    parse_stderr(&assert.get_output().stderr, root)
}

/// Run `plan propose --from <body>` expecting success and return the
/// parsed `--format json` stdout summary.
fn propose_from_ok(root: &Path, body: &str) -> Value {
    let response = write_response(root, body);
    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "plan", "propose", "--from"])
        .arg(&response)
        .assert()
        .success();
    parse_stdout(&assert.get_output().stdout, root)
}

// -- dry-run request envelope goldens --------------------------------

#[test]
fn propose_dry_run_n1_request_golden() {
    // N=1: the sole regular project is synthesised from `project.yaml`
    // (`test-proj` → `omnia@v1`); one `intent` lead surfaces.
    let project = Project::init();
    project.seed_plan(PROPOSE_PLAN_N1);
    seed_discovery(project.root(), PROPOSE_DISCOVERY_N1);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "propose", "--dry-run"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["kind"], "request");
    assert_eq!(actual["projects"].as_array().expect("projects").len(), 1);
    assert_eq!(actual["projects"][0]["name"], "test-proj");
    assert_eq!(actual["projects"][0]["target"], "omnia@v1");
    assert_eq!(actual["leads"].as_array().expect("leads").len(), 1);
    assert_eq!(actual["leads"][0]["source"], "intent");
    assert_eq!(actual["leads"][0]["lead"], "fix-typo");

    // The plan is untouched by --dry-run.
    assert_eq!(fs::read_to_string(project.plan_path()).expect("read plan"), PROPOSE_PLAN_N1);

    assert_golden("propose-dry-run-n1-request.json", actual);
}

#[test]
fn propose_dry_run_hub_request_golden() {
    // Hub: the registry's two projects and four leads across two
    // sources project verbatim into the request envelope.
    let tmp = hub_project(PROPOSE_REGISTRY_HUB, PROPOSE_DISCOVERY_HUB, PROPOSE_PLAN_HUB);

    let assert = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "plan", "propose", "--dry-run"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, tmp.path());

    assert_eq!(actual["kind"], "request");
    let projects = actual["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0]["name"], "identity-contracts");
    assert_eq!(projects[0]["target"], "contracts@v1");
    assert_eq!(projects[1]["name"], "identity-service");
    let leads = actual["leads"].as_array().expect("leads array");
    assert_eq!(leads.len(), 4);

    assert_golden("propose-dry-run-hub-request.json", actual);
}

// -- `--from` happy-path goldens -------------------------------------

#[test]
fn propose_from_n1_auto_bind_golden() {
    let project = Project::init();
    project.seed_plan(PROPOSE_PLAN_N1);
    seed_discovery(project.root(), PROPOSE_DISCOVERY_N1);

    let actual = propose_from_ok(project.root(), PROPOSE_RESPONSE_N1);
    assert_eq!(actual["plan"]["name"], "demo");
    assert_eq!(actual["slice-count"], 1);
    assert_eq!(actual["scope-count"], 1);
    assert_eq!(actual["slice-names"], serde_json::json!(["fix-typo"]));
    assert_golden("propose-from-n1-summary.json", actual);

    // The projected plan: one slice, target derived from the
    // auto-bound project, structured source binding.
    let plan = Plan::load(&project.plan_path()).expect("load plan");
    assert_eq!(plan.entries.len(), 1);
    let entry = &plan.entries[0];
    assert_eq!(entry.name, "fix-typo");
    assert_eq!(entry.project.as_deref(), Some("test-proj"));
    assert_eq!(entry.target.as_ref().map(ToString::to_string), Some("omnia@v1".to_string()));
    assert_eq!(entry.sources.len(), 1);
    assert_eq!(entry.sources[0].source(), "intent");
    assert_eq!(entry.sources[0].lead("fix-typo"), "fix-typo");
}

#[test]
fn propose_from_fan_out_golden() {
    let tmp = hub_project(PROPOSE_REGISTRY_HUB, PROPOSE_DISCOVERY_HUB, PROPOSE_PLAN_HUB);

    let actual = propose_from_ok(tmp.path(), PROPOSE_RESPONSE_FANOUT);
    assert_eq!(actual["plan"]["name"], "identity-revamp");
    assert_eq!(actual["slice-count"], 3);
    assert_eq!(actual["scope-count"], 2);
    assert_eq!(
        actual["slice-names"],
        serde_json::json!(["identity-contracts", "identity-service", "password-reset"])
    );
    assert_golden("propose-from-fan-out-summary.json", actual);

    let plan = Plan::load(&tmp.path().join("plan.yaml")).expect("load plan");
    let names: Vec<&str> = plan.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["identity-contracts", "identity-service", "password-reset"]);

    let projects: Vec<Option<&str>> = plan.entries.iter().map(|e| e.project.as_deref()).collect();
    assert_eq!(
        projects,
        [Some("identity-contracts"), Some("identity-service"), Some("identity-service")]
    );

    let targets: Vec<Option<String>> =
        plan.entries.iter().map(|e| e.target.as_ref().map(ToString::to_string)).collect();
    assert_eq!(
        targets,
        [
            Some("contracts@v1".to_string()),
            Some("omnia@v1".to_string()),
            Some("omnia@v1".to_string())
        ]
    );

    // The fan-out slice carries both matched sources, structured.
    assert_eq!(plan.entries[0].sources.len(), 2);
    assert_eq!(plan.entries[0].sources[0].source(), "docs");
    assert_eq!(plan.entries[0].sources[1].source(), "legacy");
    // The 1:1 scope keeps the cross-source leads verbatim.
    assert_eq!(plan.entries[2].sources[1].source(), "legacy");
    assert_eq!(plan.entries[2].sources[1].lead("password-reset"), "reset-password");
    // depends-on resolves to a derived slice name.
    assert_eq!(plan.entries[1].depends_on, ["identity-contracts"]);
    assert!(plan.entries[0].depends_on.is_empty());
}

// -- journal tail -----------------------------------------------------

#[test]
fn propose_from_emits_paired_journal_tail() {
    let tmp = hub_project(PROPOSE_REGISTRY_HUB, PROPOSE_DISCOVERY_HUB, PROPOSE_PLAN_HUB);
    let response = write_response(tmp.path(), PROPOSE_RESPONSE_FANOUT);
    specrun()
        .current_dir(tmp.path())
        .args(["plan", "propose", "--from"])
        .arg(&response)
        .assert()
        .success();

    let raw = fs::read_to_string(tmp.path().join(".specify/journal.jsonl")).expect("read journal");
    let events: Vec<Value> = raw
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("journal line is JSON"))
        .collect();
    assert_eq!(events.len(), 2, "exactly the two reconcile events fire, got:\n{events:#?}");

    // First: plan.reconcile.agent with deduped scopes (the fan-out
    // scope contributes a single entry that carries its rationale).
    let agent = &events[0];
    assert_eq!(agent["event"], "plan.reconcile.agent");
    assert_eq!(agent["payload"]["plan-name"], "identity-revamp");
    assert_eq!(agent["payload"]["slice-count"], 3);
    let scopes = agent["payload"]["scopes"].as_array().expect("scopes array");
    assert_eq!(scopes.len(), 2, "the identity-api fan-out scope dedupes to one entry: {scopes:#?}");
    assert_eq!(scopes[0]["scope"], "identity-api");
    assert_eq!(
        scopes[0]["rationale"],
        "identity API surface matched by shared slug across docs + legacy"
    );
    assert_eq!(scopes[1]["scope"], "password-reset");

    // Then: plan.reconcile.completed with the derived names in order.
    let completed = &events[1];
    assert_eq!(completed["event"], "plan.reconcile.completed");
    assert_eq!(completed["payload"]["plan-name"], "identity-revamp");
    assert_eq!(completed["payload"]["slice-count"], 3);
    assert_eq!(
        completed["payload"]["slice-names"],
        serde_json::json!(["identity-contracts", "identity-service", "password-reset"])
    );
}

// -- negative: command-mode + response read/parse gates --------------

#[test]
fn propose_mode_required() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "propose"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(body["error"], "plan-propose-mode-required");
}

#[test]
fn propose_response_not_found() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    let missing = project.root().join("absent.json");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "propose", "--from"])
        .arg(&missing)
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(body["error"], "plan-propose-response-not-found");
}

#[test]
fn propose_response_schema_rejected() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a")]));

    // Drop the required `kind` discriminator: the envelope matches
    // neither `oneOf` branch and is rejected by the schema gate before
    // the structural deserialise.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"slices":[{"scope":"a","sources":[{"source":"docs","lead":"a"}]}]}"#,
    );
    assert_eq!(body["error"], "proposal-schema");
}

// -- negative: propagated `plan-reconcile-*` codes -------------------
//
// Each fixture isolates one invariant by keeping every earlier check in
// the firing order satisfied (RFC-29 D2 partition invariants).

#[test]
fn propose_reconcile_lead_orphan() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "real")]));

    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"ghost"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-lead-orphan");
}

#[test]
fn propose_reconcile_slice_source_collision() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // One slice names two leads from the same source.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"a"},{"source":"docs","lead":"b"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-slice-source-collision");
}

#[test]
fn propose_reconcile_fanout_source_mismatch() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // Two slices share `scope: s` but carry differing sources.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"a"}]},{"scope":"s","sources":[{"source":"docs","lead":"b"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-fanout-source-mismatch");
}

#[test]
fn propose_reconcile_partition_gap() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // The catalog carries two leads; the response covers only `a`.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"a"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-partition");
}

#[test]
fn propose_reconcile_project_orphan() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a")]));

    // The slice binds a project absent from the (sole-project) topology.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"a"}],"project":"ghost"}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-project-orphan");
}

#[test]
fn propose_reconcile_project_binding_required() {
    // Two projects offered (hub); the slice omits `project`, so the
    // kernel cannot auto-bind.
    let tmp = hub_project(
        PROPOSE_REGISTRY_HUB,
        &discovery_doc(&[("docs", "a")]),
        "name: identity-revamp\nslices: []\n",
    );

    let body = propose_from_stderr(
        tmp.path(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"a"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-project-binding-required");
}

#[test]
fn propose_reconcile_slice_duplicate() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a")]));

    // Two slices collapse to the same (scope, auto-bound project) pair.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"scope":"s","sources":[{"source":"docs","lead":"a"}]},{"scope":"s","sources":[{"source":"docs","lead":"a"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-slice-duplicate");
}

#[test]
fn propose_reconcile_slice_name_collision() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // Two distinct scopes, but both supply the explicit name `dup`.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"dup","scope":"s1","sources":[{"source":"docs","lead":"a"}]},{"name":"dup","scope":"s2","sources":[{"source":"docs","lead":"b"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-slice-name-collision");
}

#[test]
fn propose_reconcile_depends_on_cycle() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // alpha ↔ beta depend on each other.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"alpha","scope":"s1","sources":[{"source":"docs","lead":"a"}],"depends-on":["beta"]},{"name":"beta","scope":"s2","sources":[{"source":"docs","lead":"b"}],"depends-on":["alpha"]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-depends-on-cycle");
}

#[test]
fn propose_dry_run_empty_catalog() {
    // `plan-reconcile-empty-catalog` is reachable via --dry-run (no
    // surveyed leads). Under --from it is masked by lead-orphan /
    // partition, since a schema-valid response must cite at least one
    // lead against the empty catalog.
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    // Deliberately no discovery.md.

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "propose", "--dry-run"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(body["error"], "plan-reconcile-empty-catalog");
}

// -- re-propose semantics --------------------------------------------

#[test]
fn propose_re_propose_replaces_all_slices() {
    // `--from` is a wholesale projection, not a merge: a second run on a
    // still-pending plan replaces the prior slice set entirely.
    let project = Project::init();
    project.seed_plan(PROPOSE_PLAN_N1);
    seed_discovery(project.root(), PROPOSE_DISCOVERY_N1);

    propose_from_ok(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"first","scope":"first","sources":[{"source":"intent","lead":"fix-typo"}]}]}"#,
    );
    let plan_after_first = Plan::load(&project.plan_path()).expect("load plan");
    assert_eq!(
        plan_after_first.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["first"]
    );

    propose_from_ok(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"second","scope":"second","sources":[{"source":"intent","lead":"fix-typo"}]}]}"#,
    );
    let plan_after_second = Plan::load(&project.plan_path()).expect("load plan");
    assert_eq!(
        plan_after_second.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["second"],
        "the second --from wholesale-replaces the first slice set"
    );
}

#[test]
fn propose_refuses_on_approved_plan() {
    // Once the operator stamps Gate 1 (`approved`), the plan is no
    // longer replaceable and `--from` aborts.
    let project = Project::init();
    project.seed_plan(PROPOSE_PLAN_N1);
    seed_discovery(project.root(), PROPOSE_DISCOVERY_N1);

    propose_from_ok(project.root(), PROPOSE_RESPONSE_N1);
    specrun()
        .current_dir(project.root())
        .args(["plan", "transition", "demo", "approved"])
        .assert()
        .success();

    let body = propose_from_stderr(project.root(), PROPOSE_RESPONSE_N1);
    assert_eq!(body["error"], "plan-reconcile-plan-not-replaceable");
}
