//! Integration tests for `specify change *` (the umbrella orchestration
//! surface) and `specify registry *` — `change` owns the operator brief
//! at `change.md` (renamed from the pre-Phase-3.7 `initiative.md` by
//! `specify migrate change-noun`) plus the executable plan at
//! `plan.yaml`, and `registry` covers `registry.yaml`. All three
//! platform-component artifacts live at the repo root per RFC-9 §1B.
//!
//! These CLI tests stand up a fresh `.specify/` project via
//! `specify init` (mirroring `tests/slice.rs` / `tests/e2e.rs`),
//! seed `plan.yaml` at the repo root by writing YAML directly to
//! disk, and drive the CLI through `assert_cmd`. JSON shapes are
//! pinned by checked-in fixtures under `tests/fixtures/plan/`;
//! regenerate them with
//! `REGENERATE_GOLDENS=1 cargo test --test change_umbrella`.

#[cfg(test)]
mod cli {
    use std::fs;
    use std::path::{Path, PathBuf};

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::{TempDir, tempdir};

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn plan_fixtures() -> PathBuf {
        repo_root().join("tests/fixtures/plan")
    }

    fn specify() -> Command {
        Command::cargo_bin("specify").expect("cargo_bin(specify)")
    }

    /// A `.specify/` project rooted in a throwaway tempdir.
    ///
    /// Mirrors the harness in `tests/slice.rs`: run `specify init` with
    /// the in-repo Omnia capability fixture, then let the test body seed whatever
    /// `plan.yaml` / `slices/` content it needs.
    struct Project {
        _tmp: TempDir,
        root: PathBuf,
    }

    impl Project {
        fn init() -> Self {
            let tmp = tempdir().expect("tempdir");
            let root = tmp.path().to_path_buf();
            specify()
                .current_dir(&root)
                .args(["init"])
                .arg(repo_root().join("schemas").join("omnia"))
                .args(["--name", "test-proj"])
                .assert()
                .success();
            Self { _tmp: tmp, root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn plan_path(&self) -> PathBuf {
            self.root.join("plan.yaml")
        }

        /// Seed `plan.yaml` (at the repo root) with arbitrary YAML.
        /// The tests drive the file directly (not the library's
        /// `Plan::save`) for convenience and isolation from the
        /// `create` verb.
        fn seed_plan(&self, yaml: &str) {
            fs::write(self.plan_path(), yaml).expect("write plan.yaml");
        }
    }

    // -- substitution / golden comparison (mirrors tests/e2e.rs) -------

    const TEMPDIR_PLACEHOLDER: &str = "<TEMPDIR>";

    struct Sub {
        from: String,
        to: &'static str,
    }

    /// Apply the longest candidate first. On macOS the canonical
    /// tempdir path (`/private/var/folders/...`) is a superstring of
    /// the raw path (`/var/folders/...`); if we substitute the raw
    /// path first, we strip *inside* the canonical one and leave the
    /// stray `/private` prefix in the golden. Sorting by length
    /// descending avoids that.
    fn tempdir_subs(root: &Path) -> Vec<Sub> {
        let mut subs: Vec<Sub> = Vec::new();
        if let Some(raw) = root.to_str() {
            subs.push(Sub {
                from: raw.to_string(),
                to: TEMPDIR_PLACEHOLDER,
            });
        }
        if let Ok(canonical) = fs::canonicalize(root)
            && let Some(canonical_str) = canonical.to_str()
            && Some(canonical_str) != root.to_str()
        {
            subs.push(Sub {
                from: canonical_str.to_string(),
                to: TEMPDIR_PLACEHOLDER,
            });
        }
        subs.sort_by_key(|b| std::cmp::Reverse(b.from.len()));
        subs
    }

    fn strip_substitutions(value: &mut Value, subs: &[Sub]) {
        match value {
            Value::String(s) => {
                for sub in subs {
                    if s.contains(&sub.from) {
                        *s = s.replace(&sub.from, sub.to);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    strip_substitutions(item, subs);
                }
            }
            Value::Object(map) => {
                for (_k, v) in map.iter_mut() {
                    strip_substitutions(v, subs);
                }
            }
            _ => {}
        }
    }

    fn parse_stdout(stdout: &[u8], root: &Path) -> Value {
        let text = std::str::from_utf8(stdout).expect("utf8 stdout");
        let mut value: Value = serde_json::from_str(text)
            .unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"));
        strip_substitutions(&mut value, &tempdir_subs(root));
        value
    }

    /// Compare `actual` against a checked-in golden, or rewrite it when
    /// `REGENERATE_GOLDENS=1` is set. Mirrors `tests/e2e.rs`.
    #[allow(clippy::needless_pass_by_value)]
    fn assert_golden(name: &str, actual: Value) {
        let golden_path = plan_fixtures().join(name);
        let rendered = serde_json::to_string_pretty(&actual).expect("pretty json");

        if std::env::var_os("REGENERATE_GOLDENS").is_some() {
            fs::create_dir_all(plan_fixtures()).expect("mkdir plan fixtures");
            fs::write(&golden_path, format!("{rendered}\n")).expect("write golden");
            return;
        }

        let expected_raw = fs::read_to_string(&golden_path).unwrap_or_else(|err| {
            panic!(
                "golden {} missing ({err}); regenerate via REGENERATE_GOLDENS=1 cargo test --test change_umbrella",
                golden_path.display()
            )
        });
        let expected: Value = serde_json::from_str(&expected_raw)
            .unwrap_or_else(|err| panic!("golden {} is not JSON: {err}", golden_path.display()));

        assert_eq!(
            actual,
            expected,
            "stdout diverged from golden {}\n--- actual ---\n{rendered}\n--- expected ---\n{expected_raw}",
            golden_path.display()
        );
    }

    // -- test seeds --------------------------------------------------------

    const CLEAN_PLAN: &str = "\
name: demo
changes:
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
changes:
  - name: foo
    project: default
    status: pending
  - name: foo
    project: default
    status: pending
";

    const A_DONE_B_PENDING: &str = "\
name: demo
changes:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: pending
";

    const A_IN_PROGRESS: &str = "\
name: demo
changes:
  - name: a
    project: default
    status: in-progress
";

    const ALL_DONE: &str = "\
name: demo
changes:
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
changes:
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
changes:
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
changes:
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

changes:
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
    fn change_plan_validate_clean_plan_text() {
        let project = Project::init();
        project.seed_plan(CLEAN_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "validate"])
            .assert()
            .success();
        assert_eq!(assert.get_output().status.code(), Some(0));

        let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
        // No ERROR-level lines on a clean plan.
        assert!(
            !stdout.contains("ERROR"),
            "clean plan must not print any ERROR lines, got:\n{stdout}"
        );
    }

    #[test]
    fn change_plan_validate_clean_plan_json() {
        let project = Project::init();
        project.seed_plan(CLEAN_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "validate"])
            .assert()
            .success();
        assert_eq!(assert.get_output().status.code(), Some(0));

        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["passed"], true);
        assert_eq!(actual["results"], Value::Array(vec![]));
        assert_golden("validate-clean.json", actual);
    }

    #[test]
    fn plan_validate_tolerates_in_progress_with_no_change_dir() {
        // Transient window: `specify change transition <name> in-progress`
        // can run a moment before `.specify/slices/<name>/` exists.
        // `specify plan validate` must surface a *warning* (not an
        // error) so `passed == true` and skills don't stall on start-up.
        let project = Project::init();
        project.seed_plan(A_IN_PROGRESS);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "validate"])
            .assert()
            .success();
        assert_eq!(
            assert.get_output().status.code(),
            Some(0),
            "warning-only validate must exit 0 (EXIT_SUCCESS)"
        );

        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
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
        assert_eq!(matching[0]["level"], "warning");
        assert_eq!(matching[0]["entry"], "a");
    }

    #[test]
    fn change_plan_validate_with_errors_json() {
        let project = Project::init();
        project.seed_plan(DUPLICATE_NAME_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "validate"])
            .assert()
            .failure();
        assert_eq!(
            assert.get_output().status.code(),
            Some(2),
            "duplicate-name must exit 2 (EXIT_VALIDATION_FAILED)"
        );

        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["passed"], false);
        let results = actual["results"].as_array().expect("results array");
        assert!(
            results.iter().any(|r| r["code"] == "duplicate-name" && r["level"] == "error"),
            "expected a duplicate-name error, got: {results:#?}"
        );
        assert_golden("validate-duplicate-name.json", actual);
    }

    // -- next --------------------------------------------------------------

    #[test]
    fn change_plan_next_picks_first_pending_text() {
        let project = Project::init();
        project.seed_plan(A_DONE_B_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "next"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
        assert_eq!(stdout, "b\n", "text next should be bare '<name>\\n', got: {stdout:?}");
    }

    #[test]
    fn change_plan_next_picks_first_pending_json() {
        let project = Project::init();
        project.seed_plan(A_DONE_B_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "next"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
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
    fn change_plan_next_reports_in_progress() {
        let project = Project::init();
        project.seed_plan(A_IN_PROGRESS);

        let text = specify()
            .current_dir(project.root())
            .args(["change", "plan", "next"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert!(stdout.contains('a'), "text output should mention 'a': {stdout:?}");

        let json = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "next"])
            .assert()
            .success();
        let actual = parse_stdout(&json.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["next"], Value::Null);
        assert_eq!(actual["reason"], "in-progress");
        assert_eq!(actual["active"], "a");
        assert_golden("next-in-progress.json", actual);
    }

    #[test]
    fn change_plan_next_all_done_text() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);

        let text = specify()
            .current_dir(project.root())
            .args(["change", "plan", "next"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert_eq!(stdout, "All changes done.\n");

        let json = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "next"])
            .assert()
            .success();
        let actual = parse_stdout(&json.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["reason"], "all-done");
        assert_eq!(actual["next"], Value::Null);
        assert_eq!(actual["active"], Value::Null);
        assert_golden("next-all-done.json", actual);
    }

    #[test]
    fn change_plan_next_stuck_when_deps_unmet() {
        let project = Project::init();
        project.seed_plan(STUCK_PLAN);

        let json = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "next"])
            .assert()
            .success();
        let actual = parse_stdout(&json.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["reason"], "stuck");
        assert_eq!(actual["next"], Value::Null);
        assert_eq!(actual["active"], Value::Null);
        assert_golden("next-stuck.json", actual);
    }

    // -- status ------------------------------------------------------------

    #[test]
    fn change_plan_status_renders_counts_and_topo_order_json() {
        let project = Project::init();
        project.seed_plan(PLATFORM_V2_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "status"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema-version"], 3);
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
    fn change_plan_status_on_cycle_falls_back_to_list_order() {
        let project = Project::init();
        project.seed_plan(CYCLE_PLAN);

        let output = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "status"])
            .assert()
            .success();

        let actual = parse_stdout(&output.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
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
    fn change_plan_status_surfaces_status_reason_on_failed_entry() {
        let project = Project::init();
        project.seed_plan(FAILED_WITH_REASON);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "status"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        let failed = actual["failed"].as_array().expect("failed array");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["name"], "a");
        assert_eq!(failed[0]["reason"], "boom");
    }

    #[test]
    fn change_plan_status_missing_plan_file_errors() {
        let project = Project::init();
        // Deliberately do NOT seed plan.yaml.

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "status"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
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
changes: []
";

    const SINGLE_PENDING: &str = "\
name: demo
changes:
  - name: foo
    project: default
    status: pending
";

    const SINGLE_IN_PROGRESS: &str = "\
name: demo
changes:
  - name: foo
    project: default
    status: in-progress
";

    const SINGLE_DONE: &str = "\
name: demo
changes:
  - name: foo
    project: default
    status: done
";

    const WITH_DESCRIPTION: &str = "\
name: demo
changes:
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
            .args(["--format", "json", "change", "plan", "add", "foo", "--schema", "contracts@v1"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema-version"], 3);
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
            .args(["change", "plan", "add", "foo", "--schema", "contracts@v1"])
            .assert()
            .success();

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "add", "foo", "--schema", "contracts@v1"])
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
            .args(["change", "plan", "add", "NotKebab", "--schema", "contracts@v1"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));

        let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
        assert!(!saved.contains("NotKebab"), "invalid name must not land in the plan:\n{saved}");
    }

    // -- plan amend -------------------------------------------------------

    #[test]
    fn change_plan_amend_replaces_depends_on() {
        let project = Project::init();
        project.seed_plan(
            "\
name: demo
changes:
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
                "change",
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
    fn change_plan_amend_clear_description() {
        let project = Project::init();
        project.seed_plan(WITH_DESCRIPTION);

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "amend", "foo", "--description", ""])
            .assert()
            .success();

        let saved = fs::read_to_string(project.plan_path()).expect("read");
        assert!(
            !saved.contains("description: original"),
            "original description should be gone:\n{saved}"
        );
    }

    #[test]
    fn change_plan_amend_leave_field_alone() {
        let project = Project::init();
        project.seed_plan(WITH_DESCRIPTION);

        // --depends-on (clear) but no --description; description must stay.
        specify()
            .current_dir(project.root())
            .args(["change", "plan", "amend", "foo", "--depends-on"])
            .assert()
            .success();

        let saved = fs::read_to_string(project.plan_path()).expect("read");
        assert!(
            saved.contains("description: original"),
            "description should be preserved:\n{saved}"
        );
    }

    #[test]
    fn change_plan_amend_on_missing_entry_fails() {
        let project = Project::init();
        project.seed_plan(SINGLE_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "amend", "nope", "--description", "x"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
        assert!(
            stderr.contains("no change named"),
            "stderr should mention missing change, got: {stderr:?}"
        );
    }

    // -- plan transition --------------------------------------------------

    #[test]
    fn change_plan_transition_happy_path_text() {
        let project = Project::init();
        project.seed_plan(SINGLE_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "transition", "foo", "in-progress"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
        assert!(stdout.contains("pending"), "text output should mention 'pending': {stdout:?}");
        assert!(
            stdout.contains("in-progress"),
            "text output should mention 'in-progress': {stdout:?}"
        );
    }

    #[test]
    fn change_plan_transition_legal_edge_json() {
        let project = Project::init();
        project.seed_plan(SINGLE_IN_PROGRESS);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "transition", "foo", "done"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["entry"]["name"], "foo");
        assert_eq!(actual["entry"]["status"], "done");
        assert_eq!(actual["entry"]["status-reason"], Value::Null);

        assert_golden("transition-in-progress-to-done.json", actual);
    }

    #[test]
    fn change_plan_transition_rejects_illegal_edge() {
        let project = Project::init();
        project.seed_plan(SINGLE_DONE);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "transition", "foo", "pending"])
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
    fn change_plan_transition_happy_path_json_pending_to_in_progress() {
        let project = Project::init();
        project.seed_plan(SINGLE_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "transition", "foo", "in-progress"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["entry"]["status"], "in-progress");
        assert_eq!(actual["entry"]["status-reason"], Value::Null);

        assert_golden("transition-pending-to-in-progress.json", actual);
    }

    #[test]
    fn change_plan_transition_reason_on_failed() {
        let project = Project::init();
        project.seed_plan(SINGLE_IN_PROGRESS);

        let assert = specify()
            .current_dir(project.root())
            .args([
                "--format",
                "json",
                "change",
                "plan",
                "transition",
                "foo",
                "failed",
                "--reason",
                "boom",
            ])
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
    fn change_plan_transition_rejects_reason_on_in_progress_target() {
        let project = Project::init();
        project.seed_plan(SINGLE_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "transition", "foo", "in-progress", "--reason", "x"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
        assert!(stderr.contains("--reason"), "stderr should mention '--reason', got: {stderr:?}");
    }

    #[test]
    fn change_plan_transition_clears_reason_on_pending_reentry() {
        let project = Project::init();
        project.seed_plan(
            "\
name: demo
changes:
  - name: foo
    project: default
    status: failed
    status-reason: boom
",
        );

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "transition", "foo", "pending"])
            .assert()
            .success();

        let saved = fs::read_to_string(project.plan_path()).expect("read");
        assert!(
            !saved.contains("status-reason: boom"),
            "status-reason should be cleared:\n{saved}"
        );
        assert!(saved.contains("status: pending"), "status should be pending:\n{saved}");
    }

    // -- human-driven replay (RFC-2 §"The Loop (Human-Driven)") -----------

    #[test]
    fn change_plan_human_replay_matches_fixture() {
        let project = Project::init();
        project.seed_plan(
            "\
name: demo
changes:
  - name: user-registration
    project: default
    status: done
",
        );

        specify()
            .current_dir(project.root())
            .args([
                "change", "plan",
                "add",
                "registration-duplicate-email-crash",
                "--schema",
                "contracts@v1",
                "--description",
                "Duplicate email submission returns 500 instead of 409. Modifies user-registration.",
            ])
            .assert()
            .success();

        specify()
            .current_dir(project.root())
            .args([
                "change",
                "plan",
                "transition",
                "registration-duplicate-email-crash",
                "in-progress",
            ])
            .assert()
            .success();

        specify()
            .current_dir(project.root())
            .args([
                "change",
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
            .args(["change", "plan", "transition", "registration-duplicate-email-crash", "done"])
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
                "fixture {} missing ({err}); regenerate via REGENERATE_GOLDENS=1 cargo test --test change_umbrella",
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

    // -- plan create (L3.A) -----------------------------------------------

    /// Build a blank `Project` via `specify init` and then delete the
    /// auto-created `plan.yaml` (if any) so `specify plan create` is
    /// exercised against a clean slate.
    fn init_without_plan() -> Project {
        let project = Project::init();
        let _ = fs::remove_file(project.plan_path());
        project
    }

    #[test]
    fn plan_create_creates_empty_plan_json() {
        let project = init_without_plan();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "create", "my-change"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["plan"]["name"], "my-change");
        let path_str = actual["plan"]["path"].as_str().expect("plan.path string");
        assert!(
            path_str.ends_with("/plan.yaml"),
            "plan.path should end with /plan.yaml at the repo root, got: {path_str}"
        );

        assert!(project.plan_path().exists(), "plan.yaml should be created");
        let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
        assert!(saved.contains("name: my-change"), "plan missing name:\n{saved}");
        // Empty maps/vecs serialise with either `{}`/`[]` or are omitted
        // via serde's default — either way, no actual source/change entries.
        assert!(!saved.contains("- name:"), "plan should have no change entries:\n{saved}");

        assert_golden("init-success.json", actual);
    }

    #[test]
    fn plan_create_with_sources_roundtrips() {
        let project = init_without_plan();

        specify()
            .current_dir(project.root())
            .args([
                "change",
                "plan",
                "create",
                "big",
                "--source",
                "monolith=/tmp/legacy",
                "--source",
                "orders=git@github.com:org/orders.git",
            ])
            .assert()
            .success();

        let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
        assert!(saved.contains("name: big"), "plan missing name:\n{saved}");
        assert!(saved.contains("monolith: /tmp/legacy"), "plan missing monolith source:\n{saved}");
        assert!(
            saved.contains("orders: git@github.com:org/orders.git"),
            "plan missing orders source:\n{saved}"
        );
    }

    #[test]
    fn plan_create_refuses_when_plan_exists() {
        let project = Project::init();
        project.seed_plan(EMPTY_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "create", "other"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
        assert!(
            stderr.contains("specify change plan archive"),
            "stderr should suggest `specify change plan archive`, got: {stderr:?}"
        );

        let saved = fs::read_to_string(project.plan_path()).expect("read plan.yaml");
        assert!(
            saved.contains("name: demo"),
            "existing plan.yaml must not be overwritten:\n{saved}"
        );
    }

    #[test]
    fn plan_create_rejects_invalid_name() {
        let project = init_without_plan();

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "create", "BadName"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
        assert!(stderr.contains("kebab-case"), "stderr should mention kebab-case, got: {stderr:?}");
        assert!(!project.plan_path().exists(), "no plan.yaml on invalid name");
    }

    #[test]
    fn plan_create_rejects_duplicate_source_key() {
        let project = init_without_plan();

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "create", "x", "--source", "a=/p1", "--source", "a=/p2"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8 stderr");
        assert!(
            stderr.contains("duplicate key"),
            "stderr should mention duplicate key, got: {stderr:?}"
        );
        assert!(!project.plan_path().exists(), "no plan.yaml on duplicate key");
    }

    #[test]
    fn plan_create_rejects_malformed_source() {
        let project = init_without_plan();

        // No `=` in the --source value → clap's value_parser rejects
        // the argument at parse time (exit code 2).
        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "create", "x", "--source", "badkey"])
            .assert()
            .failure();
        assert_eq!(
            assert.get_output().status.code(),
            Some(2),
            "clap parse errors must surface as exit code 2"
        );
        assert!(!project.plan_path().exists(), "no plan.yaml on malformed --source");
    }

    #[test]
    fn plan_create_validates_the_result() {
        let project = init_without_plan();

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "create", "fresh"])
            .assert()
            .success();

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "validate"])
            .assert()
            .success();
        assert_eq!(assert.get_output().status.code(), Some(0));
        let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
        assert!(
            !stdout.contains("ERROR"),
            "freshly-init'd plan must pass `specify plan validate` with no errors, got:\n{stdout}"
        );
    }

    // -- plan archive (L1.K) ----------------------------------------------

    fn today_yyyymmdd() -> String {
        chrono::Utc::now().format("%Y%m%d").to_string()
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
    fn change_plan_archive_happy_path_text() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .success();
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
    fn change_plan_archive_happy_path_json() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "archive"])
            .assert()
            .success();
        let mut actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema-version"], 3);
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
    fn change_plan_archive_refuses_without_force_on_pending_entries() {
        let project = Project::init();
        project.seed_plan(A_DONE_B_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .failure();
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
    fn change_plan_archive_refuses_json_lists_entries() {
        let project = Project::init();
        project.seed_plan(A_DONE_B_PENDING);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "archive"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));

        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["schema-version"], 3);
        assert_eq!(actual["error"], "plan-has-outstanding-work");
        let entries = actual["entries"].as_array().expect("entries array");
        let names: Vec<&str> = entries.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(names, ["b"]);

        assert_golden("archive-outstanding-work.json", actual);
    }

    #[test]
    fn change_plan_archive_with_force_on_pending_succeeds() {
        let project = Project::init();
        project.seed_plan(A_DONE_B_PENDING);

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive", "--force"])
            .assert()
            .success();

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
    fn change_plan_archive_filename_is_kebab_plan_name_plus_yyyymmdd() {
        let project = Project::init();
        project.seed_plan(
            "\
name: my-change
changes: []
",
        );

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .success();

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
    fn change_plan_archive_refuses_when_destination_exists() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);

        let dest_dir = archive_dir(&project);
        fs::create_dir_all(&dest_dir).expect("mkdir archive dir");
        let dest = dest_dir.join(format!("demo-{}.yaml", today_yyyymmdd()));
        fs::write(&dest, "prior: content\n").expect("seed prior archive");

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .failure();
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
    fn change_plan_archive_missing_plan_file_errors() {
        let project = Project::init();
        // Deliberately do NOT seed plan.yaml.

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .failure();
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
    fn change_plan_archive_co_moves_working_dir_json() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);
        let working_dir = seed_working_dir(
            &project,
            "demo",
            &[("discovery.md", b"# discovery\n"), ("proposal.md", b"# proposal\n")],
        );

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "archive"])
            .assert()
            .success();
        let mut actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema-version"], 3);
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
        assert_eq!(
            fs::read_to_string(archived_dir.join("proposal.md")).expect("read"),
            "# proposal\n"
        );

        strip_date_stamps(&mut actual);
        assert_golden("archive-success-with-working-dir.json", actual);
    }

    #[test]
    fn change_plan_archive_no_working_dir_json() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "archive"])
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
    fn change_plan_archive_co_move_destination_collision_halts_before_moving_plan() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);
        let working_dir = seed_working_dir(&project, "demo", &[("notes.md", b"# notes\n")]);

        // Pre-create the co-move destination only; the plan.yaml
        // archive destination is clear, so this hits the working-dir
        // preflight specifically.
        let dest_dir = archive_dir(&project).join(format!("demo-{}", today_yyyymmdd()));
        fs::create_dir_all(&dest_dir).expect("seed collision dir");

        let assert = specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .failure();
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
    fn change_plan_lock_acquire_then_release_cycles_cleanly() {
        let project = Project::init();

        // Use a stable agent-session PID so release can authenticate. We
        // pick the test process's own PID — guaranteed alive for the
        // duration of the test.
        let our_pid = std::process::id().to_string();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "lock", "acquire", "--pid", &our_pid])
            .assert()
            .success();
        let acquired = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(acquired["held"], true);
        assert_eq!(acquired["pid"], std::process::id());
        assert_eq!(acquired["already-held"], false);
        assert_eq!(acquired["reclaimed-stale-pid"], Value::Null);

        assert!(lock_path(&project).exists(), "lockfile must exist after acquire");
        let contents = fs::read_to_string(lock_path(&project)).expect("read lockfile");
        assert_eq!(contents.trim(), our_pid);

        let release_assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "lock", "release", "--pid", &our_pid])
            .assert()
            .success();
        let released = parse_stdout(&release_assert.get_output().stdout, project.root());
        assert_eq!(released["result"], "removed");
        assert_eq!(released["pid"], std::process::id());

        assert!(!lock_path(&project).exists(), "lockfile must be gone after release");
    }

    #[test]
    fn change_plan_lock_acquire_refuses_when_another_live_pid_stamped() {
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
            .args([
                "--format",
                "json",
                "change",
                "plan",
                "lock",
                "acquire",
                "--pid",
                &contender_pid,
            ])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));

        let value: Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("json stdout");
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
    fn change_plan_lock_status_when_held() {
        let project = Project::init();
        let our_pid = std::process::id().to_string();

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "lock", "acquire", "--pid", &our_pid])
            .assert()
            .success();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "lock", "status"])
            .assert()
            .success();
        let value = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(value["held"], true);
        assert_eq!(value["pid"], std::process::id());
        assert_eq!(value["stale"], false);

        // Text form for the same state — `held by pid <n>`.
        let text = specify()
            .current_dir(project.root())
            .args(["change", "plan", "lock", "status"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert!(
            stdout.contains("held by pid"),
            "text status should say 'held by pid …', got: {stdout:?}"
        );
    }

    #[test]
    fn change_plan_lock_status_when_absent() {
        let project = Project::init();
        // Deliberately do NOT call acquire — no stamp on disk.

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "lock", "status"])
            .assert()
            .success();
        let value = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(value["held"], false);
        assert_eq!(value["pid"], Value::Null);
        assert_eq!(value["stale"], Value::Null);

        let text = specify()
            .current_dir(project.root())
            .args(["change", "plan", "lock", "status"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert_eq!(stdout.trim(), "no lock");
    }

    // ---- Registry (RFC-3a C12) ----

    #[test]
    fn registry_load_from_tempdir() {
        use specify_registry::Registry;

        let project = Project::init();
        let registry_path = project.root().join("registry.yaml");
        fs::write(
            &registry_path,
            "version: 1\n\
             projects:\n\
             \x20\x20- name: traffic\n\
             \x20\x20\x20\x20url: .\n\
             \x20\x20\x20\x20schema: omnia@v1\n",
        )
        .expect("write registry.yaml");

        let loaded =
            Registry::load(project.root()).expect("registry parses").expect("registry present");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "traffic");
        assert_eq!(loaded.projects[0].url, ".");
        assert_eq!(loaded.projects[0].schema, "omnia@v1");
        assert!(loaded.is_single_repo());
    }

    // ---- Registry CLI verbs (RFC-3a C13) ----
    //
    // `specify registry {show, validate}` — dedicated verbs
    // that isolate the same shape check the C12 hook drives through
    // `specify plan validate`. The tests below cover the full
    // matrix: absent / well-formed / malformed × show / validate ×
    // text / json.

    const REGISTRY_SINGLE: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: omnia@v1
";

    const REGISTRY_THREE: &str = "\
version: 1
projects:
  - name: monolith
    url: .
    schema: omnia@v1
    description: Core monolith service
  - name: orders
    url: ../orders
    schema: omnia@v1
    description: Order management service
  - name: payments
    url: git@github.com:org/payments.git
    schema: omnia@v1
    description: Payment processing service
";

    fn write_registry(project: &Project, body: &str) {
        fs::write(project.root().join("registry.yaml"), body).expect("write registry");
    }

    #[test]
    fn registry_show_absent() {
        let project = Project::init();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "show"])
            .assert()
            .success();
        assert_eq!(assert.get_output().status.code(), Some(0));
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["registry"], Value::Null);
        let path = actual["path"].as_str().expect("path");
        assert!(
            path.ends_with("/registry.yaml"),
            "path should point at /registry.yaml at the repo root, got: {path}"
        );
    }

    #[test]
    fn registry_show_valid() {
        let project = Project::init();
        write_registry(&project, REGISTRY_SINGLE);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "show"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        let registry = actual["registry"].as_object().expect("registry object");
        assert_eq!(registry["version"], 1);
        let projects = registry["projects"].as_array().expect("projects array");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"], "traffic");
        assert_eq!(projects[0]["url"], ".");
        assert_eq!(projects[0]["schema"], "omnia@v1");
    }

    #[test]
    fn registry_show_text_mode() {
        let project = Project::init();
        write_registry(&project, REGISTRY_SINGLE);

        let assert =
            specify().current_dir(project.root()).args(["registry", "show"]).assert().success();
        let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
        for fragment in ["version: 1", "name: traffic", "url: .", "schema: omnia@v1"] {
            assert!(
                stdout.contains(fragment),
                "text show output should mention `{fragment}`, got:\n{stdout}"
            );
        }
    }

    #[test]
    fn registry_show_malformed() {
        let project = Project::init();
        write_registry(&project, "version: 2\nprojects: []\n");

        let assert =
            specify().current_dir(project.root()).args(["registry", "show"]).assert().failure();
        assert_ne!(assert.get_output().status.code(), Some(0));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
        assert!(
            stderr.contains("registry.yaml"),
            "stderr should mention registry.yaml, got: {stderr:?}"
        );
    }

    #[test]
    fn registry_validate_absent() {
        let project = Project::init();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "validate"])
            .assert()
            .success();
        assert_eq!(assert.get_output().status.code(), Some(0));
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["registry"], Value::Null);
        assert_eq!(actual["ok"], true);

        let text =
            specify().current_dir(project.root()).args(["registry", "validate"]).assert().success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert!(
            stdout.contains("no registry declared"),
            "text validate should say 'no registry declared', got: {stdout:?}"
        );
    }

    #[test]
    fn registry_validate_well_formed() {
        let project = Project::init();
        write_registry(&project, REGISTRY_SINGLE);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "validate"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["ok"], true);
        let registry = actual["registry"].as_object().expect("registry object");
        assert_eq!(registry["version"], 1);
    }

    #[test]
    fn registry_validate_multi_project_well_formed() {
        let project = Project::init();
        write_registry(&project, REGISTRY_THREE);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "validate"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["ok"], true);
        let projects = actual["registry"]["projects"].as_array().expect("projects array");
        assert_eq!(projects.len(), 3);
    }

    #[test]
    fn registry_validate_malformed_version() {
        let project = Project::init();
        write_registry(&project, "version: 2\nprojects: []\n");

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "validate"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(2));
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["ok"], false);
        assert_eq!(actual["kind"], "config");
        let msg = actual["error"].as_str().expect("error string");
        assert!(msg.contains("version"), "error should mention version, got: {msg}");
        assert!(msg.contains("registry.yaml"), "error should mention registry.yaml, got: {msg}");
    }

    #[test]
    fn registry_validate_malformed_duplicate_name() {
        let project = Project::init();
        write_registry(
            &project,
            "\
version: 1
projects:
  - name: dup
    url: .
    schema: omnia@v1
  - name: dup
    url: ../other
    schema: omnia@v1
",
        );

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "registry", "validate"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(2));
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["ok"], false);
        let msg = actual["error"].as_str().expect("error string");
        assert!(msg.contains("duplicate"), "error should mention duplicate, got: {msg}");
    }

    #[test]
    fn registry_validate_malformed_non_kebab() {
        let project = Project::init();
        write_registry(
            &project,
            "\
version: 1
projects:
  - name: NotKebab
    url: .
    schema: omnia@v1
",
        );

        let assert =
            specify().current_dir(project.root()).args(["registry", "validate"]).assert().failure();
        assert_eq!(assert.get_output().status.code(), Some(2));
    }

    #[test]
    fn registry_validate_unknown_top_level_key() {
        let project = Project::init();
        write_registry(&project, "version: 1\nversions: 2\nprojects: []\n");

        let assert =
            specify().current_dir(project.root()).args(["registry", "validate"]).assert().failure();
        assert_eq!(assert.get_output().status.code(), Some(2));
    }

    /// Plan "Done when" criterion: on a scaffolded project with no
    /// registry, `specify registry validate` exits 0.
    #[test]
    fn registry_validate_on_bare_repo_green() {
        let project = Project::init();
        assert!(
            !project.root().join("registry.yaml").exists(),
            "bare repo must not have a registry"
        );
        specify().current_dir(project.root()).args(["registry", "validate"]).assert().success();
    }

    // ---- Change brief CLI verbs (RFC-3a C14, RFC-13 chunk 3.7) ----
    //
    // `specify change {create, show}` — scaffolds or prints
    // `change.md` (at the repo root). The on-disk filename was
    // `initiative.md` pre-RFC-13 chunk 3.7; `specify migrate
    // change-noun` is the operator's path off the legacy filename.
    // Template byte-stability is the key contract: `create` must
    // produce the same bytes every time so operators can diff against
    // the RFC-matching golden.

    /// Byte-for-byte golden for `specify change create
    /// traffic-modernisation`. Kept in-source (not a fixture file) so
    /// the assertion is a trivial `assert_eq!` against literal bytes
    /// — the plan's "Done when" criterion.
    ///
    /// RFC-13 chunk 3.7 refreshed the prose to name the artefact a
    /// "change" (matching the new filename and the surface verbs).
    const TRAFFIC_BRIEF_GOLDEN: &str = "\
---
name: traffic-modernisation
inputs: []
---

# Traffic modernisation

<!-- One-paragraph framing of what this change is trying to
     achieve. Plans reference this brief via `change.md`. -->
";

    fn brief_path(project: &Project) -> PathBuf {
        project.root().join("change.md")
    }

    fn legacy_brief_path(project: &Project) -> PathBuf {
        project.root().join("initiative.md")
    }

    fn write_brief(project: &Project, body: &str) {
        fs::write(brief_path(project), body).expect("write change.md");
    }

    #[test]
    fn change_create_scaffolds_canonical_file() {
        let project = Project::init();
        assert!(!brief_path(&project).exists(), "bare project must not have change.md");

        specify()
            .current_dir(project.root())
            .args(["change", "create", "traffic-modernisation"])
            .assert()
            .success();

        let on_disk = fs::read_to_string(brief_path(&project)).expect("read change.md");
        assert_eq!(on_disk, TRAFFIC_BRIEF_GOLDEN);
    }

    #[test]
    fn change_create_json_response() {
        let project = Project::init();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "create", "my-change"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["action"], "init");
        assert_eq!(actual["ok"], true);
        assert_eq!(actual["name"], "my-change");
        assert!(
            actual["path"].as_str().expect("path string").ends_with("/change.md"),
            "path should point at the brief, got: {}",
            actual["path"]
        );
    }

    #[test]
    fn change_create_refuses_when_file_exists() {
        let project = Project::init();
        write_brief(&project, "---\nname: pre-existing\n---\n\nhands off\n");

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "create", "pre-existing"])
            .assert()
            .failure();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["action"], "init");
        assert_eq!(actual["ok"], false);
        assert_eq!(actual["error"], "already-exists");

        // And the file must be untouched.
        let on_disk = fs::read_to_string(brief_path(&project)).expect("read");
        assert_eq!(on_disk, "---\nname: pre-existing\n---\n\nhands off\n");
    }

    /// RFC-13 chunk 3.7: when only the pre-Phase-3.7 `initiative.md`
    /// exists, `specify change create` refuses with the loud
    /// `change-brief-became-change-md` diagnostic rather than silently
    /// minting a `change.md` alongside the legacy file. The operator
    /// path is `specify migrate change-noun`.
    #[test]
    fn change_create_refuses_when_only_legacy_brief_present() {
        let project = Project::init();
        fs::write(legacy_brief_path(&project), "---\nname: legacy\n---\n")
            .expect("seed initiative.md");

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "create", "demo"])
            .assert()
            .failure();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["error"], "change-brief-became-change-md");
        // The legacy file must remain untouched and `change.md` must
        // not have been minted.
        assert!(legacy_brief_path(&project).exists(), "legacy file must remain");
        assert!(!brief_path(&project).exists(), "modern file must not be created");
    }

    #[test]
    fn change_create_rejects_non_kebab_name() {
        let project = Project::init();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "create", "NotKebab"])
            .assert()
            .failure();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["error"], "config");
        let msg = actual["message"].as_str().expect("message");
        assert!(msg.contains("kebab-case"), "msg should mention kebab-case: {msg}");
        assert!(msg.contains("NotKebab"), "msg should mention the bad name: {msg}");
        assert!(!brief_path(&project).exists(), "no file should have been created");
    }

    #[test]
    fn change_show_absent() {
        let project = Project::init();

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "show"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["brief"], Value::Null);
        let path = actual["path"].as_str().expect("path");
        assert!(path.ends_with("/change.md"), "path should point at change.md, got: {path}");

        let text =
            specify().current_dir(project.root()).args(["change", "show"]).assert().success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert!(
            stdout.contains("no change brief declared"),
            "text show should say 'no change brief declared', got: {stdout:?}"
        );
    }

    /// RFC-13 chunk 3.7: when only the pre-Phase-3.7 `initiative.md`
    /// exists, `specify change show` refuses with the loud
    /// `change-brief-became-change-md` diagnostic and points the
    /// operator at `specify migrate change-noun`.
    #[test]
    fn change_show_refuses_when_only_legacy_brief_present() {
        let project = Project::init();
        fs::write(legacy_brief_path(&project), "---\nname: legacy\n---\n\nbody\n")
            .expect("seed initiative.md");

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "show"])
            .assert()
            .failure();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["error"], "change-brief-became-change-md");
        let msg = actual["message"].as_str().expect("message string");
        assert!(msg.contains("specify migrate change-noun"), "msg: {msg}");
        assert!(msg.contains("change.md"), "msg: {msg}");
        assert!(msg.contains("initiative.md"), "msg: {msg}");
    }

    #[test]
    fn change_show_valid_text_and_json() {
        let project = Project::init();
        write_brief(
            &project,
            "---\n\
             name: traffic-modernisation\n\
             inputs:\n\
             \x20\x20- path: ./inputs/legacy/\n\
             \x20\x20\x20\x20kind: legacy-code\n\
             ---\n\
             \n\
             # Traffic modernisation\n\
             \n\
             Prose goes here.\n",
        );

        // JSON
        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "show"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        let brief = actual["brief"].as_object().expect("brief object");
        assert_eq!(brief["frontmatter"]["name"], "traffic-modernisation");
        let inputs = brief["frontmatter"]["inputs"].as_array().expect("inputs array");
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0]["path"], "./inputs/legacy/");
        assert_eq!(inputs[0]["kind"], "legacy-code");
        assert!(
            brief["body"].as_str().expect("body").contains("# Traffic modernisation"),
            "body should contain the heading, got: {:?}",
            brief["body"]
        );

        // Text
        let text =
            specify().current_dir(project.root()).args(["change", "show"]).assert().success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        for fragment in
            ["name: traffic-modernisation", "path: ./inputs/legacy/", "kind: legacy-code"]
        {
            assert!(
                stdout.contains(fragment),
                "text show should mention `{fragment}`, got:\n{stdout}"
            );
        }
    }

    #[test]
    fn change_show_malformed_returns_error() {
        let project = Project::init();
        write_brief(&project, "---\nname: BadName\n---\n\nbody\n");

        let assert =
            specify().current_dir(project.root()).args(["change", "show"]).assert().failure();
        assert_ne!(assert.get_output().status.code(), Some(0));
        let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
        // Post-RFC-13 chunk 3.7: the parser surfaces the on-disk
        // filename (`change.md`) — `initiative.md` is the legacy
        // name covered by the `change-brief-became-change-md`
        // diagnostic, not by the kebab-case rule.
        assert!(stderr.contains("change.md"), "stderr should mention change.md, got: {stderr:?}");
        assert!(
            stderr.contains("kebab-case"),
            "stderr should mention the kebab-case rule, got: {stderr:?}"
        );
    }

    /// RFC-3a C14 archive-sweep hook: the operator brief travels with
    /// the archive. Real C33 sweep adds `workspace.md` + `slices/`;
    /// this test pins the brief half. Post-RFC-13 chunk 3.7 the
    /// brief is `change.md`.
    #[test]
    fn archive_includes_change_md() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);
        write_brief(&project, TRAFFIC_BRIEF_GOLDEN);

        specify()
            .current_dir(project.root())
            .args(["change", "plan", "archive"])
            .assert()
            .success();

        assert!(!brief_path(&project).exists(), "change.md must leave the repo root");

        let archived_dir = project
            .root()
            .join(".specify/archive/plans")
            .join(format!("demo-{}", today_yyyymmdd()));
        let archived_brief = archived_dir.join("change.md");
        assert!(
            archived_brief.exists(),
            "archived change.md missing at {}",
            archived_brief.display()
        );
        let contents = fs::read_to_string(&archived_brief).expect("read archived brief");
        assert_eq!(contents, TRAFFIC_BRIEF_GOLDEN, "archived bytes must match source bytes");
    }

    /// RFC-13 chunk 3.7: `specify change plan archive` refuses to
    /// archive when the operator brief is still on the pre-Phase-3.7
    /// filename. The archive co-moves the brief; refusing keeps the
    /// archived `<name>-<date>/` directory free of stale legacy
    /// filenames mixed with post-rename ones.
    #[test]
    fn archive_refuses_when_only_legacy_brief_present() {
        let project = Project::init();
        project.seed_plan(ALL_DONE);
        fs::write(legacy_brief_path(&project), TRAFFIC_BRIEF_GOLDEN).expect("seed initiative.md");

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "archive"])
            .assert()
            .failure();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["error"], "change-brief-became-change-md");
        // The plan must remain at the repo root and the archive
        // directory must not have been created.
        assert!(project.root().join("plan.yaml").exists(), "plan.yaml must remain");
        assert!(legacy_brief_path(&project).exists(), "legacy brief must remain");
    }

    /// `specify plan validate` surfaces a malformed `registry.yaml`
    /// alongside plan validation results — the shape-validation hook
    /// complementing the dedicated `specify registry validate`
    /// verb.
    #[test]
    fn change_plan_validate_surfaces_registry_shape_errors() {
        let project = Project::init();
        // Seed a minimal, structurally-valid plan so `change plan validate`
        // doesn't exit on the plan load itself.
        project.seed_plan("name: demo\nchanges: []\n");
        // Then stomp the registry with an illegal version.
        fs::write(project.root().join("registry.yaml"), "version: 2\nprojects: []\n")
            .expect("write bad registry");

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "change", "plan", "validate"])
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
        assert_eq!(registry_findings[0]["level"], "error");
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
            .args(["change", "create", "rfc3a-planning"])
            .assert()
            .success();
        specify()
            .current_dir(project.root())
            .args(["change", "plan", "create", "rfc3a-planning", "--source", "app=."])
            .assert()
            .success();
        specify()
            .current_dir(project.root())
            .args(["change", "plan", "validate"])
            .assert()
            .success();
    }

    #[test]
    fn rfc3a_c35_workspace_sync_absent_registry_exits_zero() {
        let project = Project::init();
        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "workspace", "sync"])
            .assert()
            .success();
        let v = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(v["synced"], false);
        assert!(v["message"].as_str().unwrap().contains("no registry"));
    }

    #[test]
    fn rfc3a_c35_workspace_sync_two_local_symlink_peers() {
        let tmp = tempdir().expect("tempdir");
        let peer = tmp.path().join("peer-proj");
        fs::create_dir_all(peer.join(".specify")).expect("peer .specify");
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).expect("root");
        specify()
            .current_dir(&root)
            .args(["init"])
            .arg(repo_root().join("schemas").join("omnia"))
            .args(["--name", "rfc3a-ws"])
            .assert()
            .success();

        let reg = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    description: Root project
  - name: beta
    url: ../peer-proj
    schema: omnia@v1
    description: Peer project
";
        fs::write(root.join("registry.yaml"), reg).expect("registry");

        specify().current_dir(&root).args(["workspace", "sync"]).assert().success();

        assert!(root.join(".specify/workspace/alpha").exists());
        assert!(root.join(".specify/workspace/beta").exists());

        let assert_st = specify()
            .current_dir(&root)
            .args(["--format", "json", "workspace", "status"])
            .assert()
            .success();
        let v = parse_stdout(&assert_st.get_output().stdout, &root);
        let slots = v["slots"].as_array().expect("slots array");
        assert_eq!(slots.len(), 2);
        let kinds: Vec<&str> = slots.iter().map(|s| s["kind"].as_str().expect("kind")).collect();
        assert!(kinds.contains(&"symlink"), "expected symlink slots, got {kinds:?}");
    }
}
