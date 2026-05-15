//! Integration tests for `specify plan *` — the top-level verb that
//! orchestrates the executable plan at `plan.yaml` (RFC-9 §1B).
//!
//! These CLI tests stand up a fresh `.specify/` project via `specify
//! init` (mirroring `tests/slice.rs` / `tests/e2e.rs`), seed
//! `plan.yaml` at the repo root by writing YAML directly to disk, and
//! drive the CLI through `assert_cmd`. JSON shapes are pinned by
//! checked-in fixtures under `tests/fixtures/plan/`; regenerate them
//! with
//! `REGENERATE_GOLDENS=1 cargo test --test plan_orchestrate`.

use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

mod common;
use common::{
    Project, assert_golden_at, omnia_schema_dir, parse_stderr, parse_stdout, repo_root, specify,
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

/// `a` failed, `b` pending depends-on `a`: neither is eligible but
/// not every entry is terminal, so `next` reports `stuck`.
const STUCK_PLAN: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: failed
    status-reason: boom
  - name: b
    project: default
    status: pending
    depends-on: [a]
";

const CYCLE_PLAN: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: pending
    depends-on: [c]
  - name: b
    project: default
    status: pending
    depends-on: [a]
  - name: c
    project: default
    status: pending
    depends-on: [b]
";

const FAILED_WITH_REASON: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: failed
    status-reason: boom
";

/// Verbatim RFC-2 §"The Plan" platform-v2 example. Used by the
/// status smoke test so status output stays pinned to the RFC
/// reference shape across L1.J–L1.L.
const PLATFORM_V2_PLAN: &str = r"name: platform-v2

sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git

slices:
  - name: user-registration
    project: platform
    sources: [monolith]
    status: done

  - name: email-verification
    project: platform
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress

  - name: registration-duplicate-email-crash
    project: platform
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
      Modifies user-registration.
    status: pending

  - name: notification-preferences
    project: platform
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending

  - name: extract-shared-validation
    project: platform
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
      Delta-targets user-registration and email-verification.
    depends-on: [email-verification]
    status: pending

  - name: product-catalog
    project: platform
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending

  - name: shopping-cart
    project: platform
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending

  - name: checkout-api
    project: platform
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.

  - name: checkout-ui
    project: platform
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
";

// -- validate ----------------------------------------------------------

#[test]
fn plan_validate_clean_text() {
    let project = Project::init();
    project.seed_plan(CLEAN_PLAN);

    let assert =
        specify().current_dir(project.root()).args(["plan", "validate"]).assert().success();
    assert_eq!(assert.get_output().status.code(), Some(0));

    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    // No ERROR-level lines on a clean plan.
    assert!(!stdout.contains("ERROR"), "clean plan must not print any ERROR lines, got:\n{stdout}");
}

#[test]
fn plan_validate_clean_json() {
    let project = Project::init();
    project.seed_plan(CLEAN_PLAN);

    let assert = specify()
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
    // `specify plan validate` must surface a *warning* (not an
    // error) so `passed == true` and skills don't stall on start-up.
    let project = Project::init();
    project.seed_plan(A_IN_PROGRESS);

    let assert = specify()
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

    let assert = specify()
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

    let assert = specify().current_dir(project.root()).args(["plan", "next"]).assert().success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert_eq!(stdout, "b\n", "text next should be bare '<name>\\n', got: {stdout:?}");
}

#[test]
fn plan_next_picks_first_pending_json() {
    let project = Project::init();
    project.seed_plan(A_DONE_B_PENDING);

    let assert = specify()
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

    let text = specify().current_dir(project.root()).args(["plan", "next"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert!(stdout.contains('a'), "text output should mention 'a': {stdout:?}");

    let json = specify()
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

    let text = specify().current_dir(project.root()).args(["plan", "next"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert_eq!(stdout, "All changes done.\n");

    let json = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    let actual = parse_stdout(&json.get_output().stdout, project.root());
    assert_eq!(actual["reason"], "all-done");
    assert_eq!(actual["next"], Value::Null);
    assert_eq!(actual["active"], Value::Null);
    assert_golden("next-all-done.json", actual);
}

#[test]
fn plan_next_stuck_when_deps_unmet() {
    let project = Project::init();
    project.seed_plan(STUCK_PLAN);

    let json = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    let actual = parse_stdout(&json.get_output().stdout, project.root());
    assert_eq!(actual["reason"], "stuck");
    assert_eq!(actual["next"], Value::Null);
    assert_eq!(actual["active"], Value::Null);
    assert_golden("next-stuck.json", actual);
}

// -- status ------------------------------------------------------------

#[test]
fn plan_status_renders_counts_and_topo() {
    let project = Project::init();
    project.seed_plan(PLATFORM_V2_PLAN);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "status"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    let counts = actual["counts"].as_object().expect("counts object");
    for key in ["done", "in-progress", "pending", "blocked", "failed", "skipped", "total"] {
        assert!(counts.contains_key(key), "counts missing key '{key}': {counts:?}");
    }
    assert_eq!(counts["done"], 1);
    assert_eq!(counts["in-progress"], 1);
    assert_eq!(counts["pending"], 6);
    assert_eq!(counts["failed"], 1);
    assert_eq!(counts["total"], 9);

    assert_eq!(actual["order"], "topological");
    let entries = actual["entries"].as_array().expect("entries array");
    let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "user-registration",
            "email-verification",
            "registration-duplicate-email-crash",
            "notification-preferences",
            "extract-shared-validation",
            "product-catalog",
            "shopping-cart",
            "checkout-api",
            "checkout-ui",
        ],
        "entries should be in RFC-2 topological order"
    );

    assert_golden("status-platform-v2.json", actual);
}

#[test]
fn plan_status_cycle_falls_back_to_list() {
    let project = Project::init();
    project.seed_plan(CYCLE_PLAN);

    let output = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "status"])
        .assert()
        .success();

    let actual = parse_stdout(&output.get_output().stdout, project.root());
    assert_eq!(actual["order"], "list", "cycle must trigger list-order fallback");

    let names: Vec<&str> = actual["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);

    let stderr = std::str::from_utf8(&output.get_output().stderr).expect("utf8 stderr");
    assert!(
        stderr.to_lowercase().contains("cycle"),
        "stderr should mention 'cycle' on fallback, got: {stderr:?}"
    );
}

#[test]
fn plan_status_surfaces_reason_on_failed() {
    let project = Project::init();
    project.seed_plan(FAILED_WITH_REASON);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "status"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    let failed = actual["failed"].as_array().expect("failed array");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["name"], "a");
    assert_eq!(failed[0]["reason"], "boom");
}

#[test]
fn plan_status_missing_file_errors() {
    let project = Project::init();
    // Deliberately do NOT seed plan.yaml.

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "status"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    // Failure envelopes are written to stderr.
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "artifact-not-found");
    assert!(
        value["message"].as_str().unwrap_or_default().contains("plan.yaml not found at"),
        "message should mention 'plan.yaml not found at', got: {}",
        value["message"]
    );
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

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "add", "foo", "--capability", "contracts@v1"])
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

    specify()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--capability", "contracts@v1"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["plan", "add", "foo", "--capability", "contracts@v1"])
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

    let assert = specify()
        .current_dir(project.root())
        .args(["plan", "add", "NotKebab", "--capability", "contracts@v1"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));

    let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(!saved.contains("NotKebab"), "invalid name must not land in the plan:\n{saved}");
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

    let assert = specify()
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

    specify()
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
    specify()
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

    let assert = specify()
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
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specify()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "in-progress"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert!(stdout.contains("pending"), "text output should mention 'pending': {stdout:?}");
    assert!(stdout.contains("in-progress"), "text output should mention 'in-progress': {stdout:?}");
}

#[test]
fn plan_transition_legal_edge_json() {
    let project = Project::init();
    project.seed_plan(SINGLE_IN_PROGRESS);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "foo", "done"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["entry"]["name"], "foo");
    assert_eq!(actual["entry"]["status"], "done");
    assert_eq!(actual["entry"]["status-reason"], Value::Null);

    assert_golden("transition-in-progress-to-done.json", actual);
}

#[test]
fn plan_transition_rejects_illegal_edge() {
    let project = Project::init();
    project.seed_plan(SINGLE_DONE);

    let assert = specify()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "pending"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(
        stderr.to_lowercase().contains("illegal") || stderr.contains("transition"),
        "stderr should mention illegal transition, got: {stderr:?}"
    );
}

#[test]
fn plan_transition_pending_to_in_progress_json() {
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "foo", "in-progress"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["entry"]["status"], "in-progress");
    assert_eq!(actual["entry"]["status-reason"], Value::Null);

    assert_golden("transition-pending-to-in-progress.json", actual);
}

#[test]
fn plan_transition_reason_on_failed() {
    let project = Project::init();
    project.seed_plan(SINGLE_IN_PROGRESS);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "transition", "foo", "failed", "--reason", "boom"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["entry"]["status"], "failed");
    assert_eq!(actual["entry"]["status-reason"], "boom");

    let saved = fs::read_to_string(project.plan_path()).expect("read");
    assert!(saved.contains("status-reason: boom"), "saved reason missing:\n{saved}");

    assert_golden("transition-in-progress-to-failed-with-reason.json", actual);
}

#[test]
fn plan_transition_rejects_reason_on_in_progress() {
    let project = Project::init();
    project.seed_plan(SINGLE_PENDING);

    let assert = specify()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "in-progress", "--reason", "x"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(stderr.contains("--reason"), "stderr should mention '--reason', got: {stderr:?}");
}

#[test]
fn plan_transition_clears_reason_on_reentry() {
    let project = Project::init();
    project.seed_plan(
        "\
name: demo
slices:
  - name: foo
    project: default
    status: failed
    status-reason: boom
",
    );

    specify()
        .current_dir(project.root())
        .args(["plan", "transition", "foo", "pending"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read");
    assert!(!saved.contains("status-reason: boom"), "status-reason should be cleared:\n{saved}");
    assert!(saved.contains("status: pending"), "status should be pending:\n{saved}");
}

// -- human-driven replay (RFC-2 §"The Loop (Human-Driven)") -----------

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

    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "registration-duplicate-email-crash",
            "--capability",
            "contracts@v1",
            "--description",
            "Duplicate email submission returns 500 instead of 409. Modifies user-registration.",
        ])
        .assert()
        .success();

    specify()
        .current_dir(project.root())
        .args(["plan", "transition", "registration-duplicate-email-crash", "in-progress"])
        .assert()
        .success();

    specify()
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

    specify()
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

// -- change create + plan validate smoke (L3.A) ----------------------
//
// `specify change create` (the merged verb that scaffolds change.md
// and plan.yaml together) gets its envelope/refusal/source coverage
// in `tests/change_create.rs`. The smoke test below confirms that a
// freshly-scaffolded plan validates cleanly out of the box, and that
// the JSON envelope produced by the merged verb matches the pinned
// golden.

#[test]
fn change_create_empty_json_matches_golden() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "create", "my-change"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());

    assert_eq!(actual["name"], "my-change");
    let plan_path = actual["plan"].as_str().expect("plan string");
    assert!(
        plan_path.ends_with("/plan.yaml"),
        "plan should end with /plan.yaml at the repo root, got: {plan_path}"
    );
    let brief_path = actual["brief"].as_str().expect("brief string");
    assert!(
        brief_path.ends_with("/change.md"),
        "brief should end with /change.md at the repo root, got: {brief_path}"
    );

    assert!(project.plan_path().exists(), "plan.yaml should be created");
    let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
    assert!(saved.contains("name: my-change"), "plan missing name:\n{saved}");
    assert!(!saved.contains("- name:"), "plan should have no change entries:\n{saved}");

    assert_golden("init-success.json", actual);
}

#[test]
fn plan_create_scaffolds_plan_only_json_matches_golden() {
    let project = Project::init();

    let assert = specify()
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
fn plan_create_refuses_to_overwrite_existing_plan() {
    let project = Project::init();
    specify().current_dir(project.root()).args(["plan", "create", "first"]).assert().success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "create", "second"])
        .assert()
        .failure();
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "already-exists");
}

#[test]
fn change_create_then_validate_passes_clean() {
    let project = Project::init();

    specify().current_dir(project.root()).args(["change", "create", "fresh"]).assert().success();

    let assert =
        specify().current_dir(project.root()).args(["plan", "validate"]).assert().success();
    assert_eq!(assert.get_output().status.code(), Some(0));
    let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
    assert!(
        !stdout.contains("ERROR"),
        "freshly-scaffolded plan must pass `specify plan validate` with no errors, got:\n{stdout}"
    );
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

    let assert = specify().current_dir(project.root()).args(["plan", "archive"]).assert().success();
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

    let assert = specify()
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

    let assert = specify().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
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

    let assert = specify()
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

    specify().current_dir(project.root()).args(["plan", "archive", "--force"]).assert().success();

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
fn plan_archive_filename_is_kebab_plus_date() {
    let project = Project::init();
    project.seed_plan(
        "\
name: my-change
slices: []
",
    );

    specify().current_dir(project.root()).args(["plan", "archive"]).assert().success();

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

    let assert = specify().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
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

    let assert = specify().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
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

    let assert = specify()
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

    let assert = specify()
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

    let assert = specify().current_dir(project.root()).args(["plan", "archive"]).assert().failure();
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

// -- plan lock {acquire, release, status} (L2.E) ----------------------

fn lock_path(project: &Project) -> PathBuf {
    project.root().join(".specify/plan.lock")
}

#[test]
fn plan_lock_acquire_release_cycles() {
    let project = Project::init();

    // Use a stable agent-session PID so release can authenticate. We
    // pick the test process's own PID — guaranteed alive for the
    // duration of the test.
    let our_pid = std::process::id().to_string();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "lock", "acquire", "--pid", &our_pid])
        .assert()
        .success();
    let acquired = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(acquired["pid"], std::process::id());
    assert_eq!(acquired["already-held"], false);
    assert_eq!(acquired["reclaimed-stale-pid"], Value::Null);
    assert_eq!(acquired.get("held"), None, "acquire body must not carry the redundant `held` flag");

    assert!(lock_path(&project).exists(), "lockfile must exist after acquire");
    let contents = fs::read_to_string(lock_path(&project)).expect("read lockfile");
    assert_eq!(contents.trim(), our_pid);

    let release_assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "lock", "release", "--pid", &our_pid])
        .assert()
        .success();
    let released = parse_stdout(&release_assert.get_output().stdout, project.root());
    assert_eq!(released["result"], "removed");
    assert_eq!(released["pid"], std::process::id());

    assert!(!lock_path(&project).exists(), "lockfile must be gone after release");
}

#[test]
fn plan_lock_acquire_refuses_on_live_pid() {
    let project = Project::init();

    // Prime with our own PID — the CLI's liveness probe will find it
    // alive (the test process is still running) and refuse to let a
    // different PID take over.
    let live_pid = std::process::id();
    fs::create_dir_all(project.root().join(".specify")).expect("mkdir .specify");
    fs::write(lock_path(&project), format!("{live_pid}\n")).expect("seed live stamp");

    // Pick any PID that isn't the test process's own PID.
    let contender_pid = if live_pid == 1 { 2 } else { 1 }.to_string();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "lock", "acquire", "--pid", &contender_pid])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));

    // Failure envelopes are written to stderr.
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json stderr");
    assert_eq!(value["error"], "driver-busy");
    assert_eq!(value["exit-code"], 1, "DriverBusy must surface the generic-failure exit code");
    let msg = value["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&format!("pid {live_pid}")),
        "message should name the holder pid {live_pid}, got: {msg}"
    );

    // Lockfile contents must be preserved — the acquire failed, so
    // the live holder stays stamped.
    let contents = fs::read_to_string(lock_path(&project)).expect("read");
    assert_eq!(contents.trim(), live_pid.to_string());
}

#[test]
fn plan_lock_status_when_held() {
    let project = Project::init();
    let our_pid = std::process::id().to_string();

    specify()
        .current_dir(project.root())
        .args(["plan", "lock", "acquire", "--pid", &our_pid])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "lock", "status"])
        .assert()
        .success();
    let value = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(value["held"], true);
    assert_eq!(value["pid"], std::process::id());
    assert_eq!(value["stale"], false);

    // Text form for the same state — `held by pid <n>`.
    let text =
        specify().current_dir(project.root()).args(["plan", "lock", "status"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert!(
        stdout.contains("held by pid"),
        "text status should say 'held by pid …', got: {stdout:?}"
    );
}

#[test]
fn plan_lock_status_when_absent() {
    let project = Project::init();
    // Deliberately do NOT call acquire — no stamp on disk.

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "lock", "status"])
        .assert()
        .success();
    let value = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(value["held"], false);
    assert_eq!(value["pid"], Value::Null);
    assert_eq!(value["stale"], Value::Null);

    let text =
        specify().current_dir(project.root()).args(["plan", "lock", "status"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert_eq!(stdout.trim(), "no lock");
}
/// `specify plan validate` surfaces a malformed `registry.yaml`
/// alongside plan validation results — the shape-validation hook
/// complementing the dedicated `specify registry validate`
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

    let assert = specify()
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

// ---- RFC-3a C35 — planning-path smoke (Stage A/B, manifest, Layer 2) ----

#[test]
fn rfc3a_c35_stage_ab_change_brief_and_plan_validate() {
    let project = Project::init();
    specify()
        .current_dir(project.root())
        .args(["change", "create", "rfc3a-planning", "--source", "app=."])
        .assert()
        .success();
    specify().current_dir(project.root()).args(["plan", "validate"]).assert().success();
}

// ---- specify plan validate health diagnostics (RFC-9 §4B) ----
//
// `plan validate` carries the four health diagnostics
// (`cycle-in-depends-on`, `orphan-source-key`, `stale-workspace-clone`,
// `unreachable-entry`) alongside its base shape rules. The integration
// tests below pin the wire-shape skill authors rely on: validate MUST
// surface every diagnostic class on a synthetic fixture that exercises
// all four, and the structured `data` payload MUST round-trip through
// the JSON envelope.

fn init_omnia_project(tmp: &TempDir) {
    specify()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();
}

#[test]
fn plan_validate_reports_all_four_health_diagnostics() {
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
             \x20\x20monolith: /tmp/legacy\n\
             \x20\x20orphaned: /tmp/elsewhere\n\
             slices:\n\
             \x20\x20- name: cyclic-a\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyclic-b]\n\
             \x20\x20- name: cyclic-b\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyclic-a]\n\
             \x20\x20- name: failed-root\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
             \x20\x20\x20\x20status: failed\n\
             \x20\x20\x20\x20status-reason: regression in upstream service\n\
             \x20\x20- name: unreachable-leaf\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [failed-root]\n\
             \x20\x20- name: orphaned-source-user\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
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
             \x20\x20\x20\x20capability: omnia@v1\n",
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
        specify().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8");
    let value: Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    let results = value["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "validate with broken plan must surface results: {value}");
    let codes: Vec<&str> = results.iter().filter_map(|r| r["code"].as_str()).collect();

    for expected in
        ["cycle-in-depends-on", "orphan-source-key", "stale-workspace-clone", "unreachable-entry"]
    {
        assert!(
            codes.contains(&expected),
            "validate must emit `{expected}` for the synthetic fixture; saw: {codes:?}"
        );
    }

    // Exit code must be ValidationFailed (2) because cycle and
    // unreachable-entry are error-severity.
    let code = output.status.code().expect("exit code");
    assert_eq!(code, 2, "error-severity diagnostics must yield exit 2, got {code}");
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
             \x20\x20orphan-key: /tmp/somewhere\n\
             slices:\n\
             \x20\x20- name: cyc-a\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyc-b]\n\
             \x20\x20- name: cyc-b\n\
             \x20\x20\x20\x20capability: omnia@v1\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyc-a]\n",
    )
    .unwrap();

    let assert =
        specify().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
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
        .find(|d| d["code"] == "orphan-source-key")
        .expect("expected orphan-source-key diagnostic");
    assert_eq!(orphan["data"]["kind"], "orphan-source");
    assert_eq!(orphan["data"]["key"], "orphan-key");
    assert_eq!(orphan["severity"], "warning");
}

#[test]
fn plan_validate_healthy_exits_zero() {
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "create", "demo"])
        .assert()
        .success();

    let assert = specify()
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
