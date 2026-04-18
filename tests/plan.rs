//! Integration tests for `schemas/plan/plan.schema.json` plus the
//! `specify plan {validate, next, status}` CLI commands.
//!
//! The schema tests are pure-library: they compile the bundled JSON
//! Schema and feed it YAML fixtures converted to `serde_json::Value`.
//!
//! The CLI tests under `mod cli` stand up a fresh `.specify/` project
//! via `specify init` (mirroring `tests/change.rs` / `tests/e2e.rs`),
//! seed `.specify/plan.yaml` by writing YAML directly to disk, and
//! drive `specify plan *` through `assert_cmd`. JSON shapes are pinned
//! by checked-in fixtures under `tests/fixtures/plan/`; regenerate
//! them with `REGENERATE_GOLDENS=1 cargo test --test plan`.

use std::fs;
use std::path::PathBuf;

use jsonschema::Validator;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

/// RFC-2 §"The Plan" `platform-v2` example, inline.
///
/// Kept inline (rather than loaded from a fixture) so the test is pinned to
/// the exact shape the RFC ships; subsequent Changes that touch the RFC must
/// also touch this constant.
const RFC_EXAMPLE: &str = r#"
name: platform-v2

sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git

changes:
  - name: user-registration
    sources: [monolith]
    status: done

  - name: email-verification
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress

  - name: registration-duplicate-email-crash
    affects: [user-registration]
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending

  - name: notification-preferences
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending

  - name: extract-shared-validation
    affects: [user-registration, email-verification]
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending

  - name: product-catalog
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending

  - name: shopping-cart
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending

  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.

  - name: checkout-ui
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
"#;

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/plan/plan.schema.json")
}

fn load_validator() -> Validator {
    let raw = fs::read_to_string(schema_path()).expect("read plan.schema.json");
    let schema: JsonValue = serde_json::from_str(&raw).expect("plan.schema.json is valid JSON");
    jsonschema::validator_for(&schema).expect("plan.schema.json compiles as a JSON Schema")
}

fn yaml_to_json(yaml: &str) -> JsonValue {
    let value: YamlValue = serde_yaml::from_str(yaml).expect("fixture parses as YAML");
    serde_json::to_value(value).expect("YAML value serialises as JSON")
}

#[test]
fn plan_schema_validates_rfc_example() {
    let validator = load_validator();
    let instance = yaml_to_json(RFC_EXAMPLE);
    let errors: Vec<String> =
        validator.iter_errors(&instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();
    assert!(errors.is_empty(), "RFC-2 example should validate cleanly; errors: {errors:#?}");
}

#[test]
fn plan_schema_rejects_unknown_status_value() {
    let validator = load_validator();
    let mutated = RFC_EXAMPLE.replacen("status: in-progress", "status: maybe", 1);
    let instance = yaml_to_json(&mutated);

    let offending_paths: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| e.instance_path().to_string())
        .filter(|p| p.starts_with("/changes/") && p.ends_with("/status"))
        .collect();

    assert!(
        !offending_paths.is_empty(),
        "unknown status should produce at least one error on /changes/*/status; got none"
    );
}

#[test]
fn plan_schema_rejects_non_kebab_name() {
    let validator = load_validator();
    let mutated = RFC_EXAMPLE.replacen("name: platform-v2", "name: Platform V2", 1);
    let instance = yaml_to_json(&mutated);

    let name_errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| e.instance_path().to_string())
        .filter(|p| p == "/name")
        .collect();

    assert!(
        !name_errors.is_empty(),
        "non-kebab-case name should produce at least one error on /name; got none"
    );
}

// ---------------------------------------------------------------------------
// CLI integration tests for `specify plan {validate, next, status}`
// ---------------------------------------------------------------------------

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
    /// Mirrors the harness in `tests/change.rs`: run `specify init` with
    /// `--schema-dir` pointed at the repo root so `schema.yaml` is
    /// always resolvable, then let the test body seed whatever
    /// `plan.yaml` / `changes/` content it needs.
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
                .args(["init", "omnia", "--schema-dir"])
                .arg(repo_root())
                .args(["--name", "test-proj"])
                .assert()
                .success();
            Project { _tmp: tmp, root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn plan_path(&self) -> PathBuf {
            self.root.join(".specify/plan.yaml")
        }

        /// Seed `.specify/plan.yaml` with arbitrary YAML. The tests
        /// drive the file directly (not the library's `Plan::save`)
        /// because `specify plan create` is out of scope for L1.I.
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
        subs.sort_by(|a, b| b.from.len().cmp(&a.from.len()));
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
                "golden {} missing ({err}); regenerate via REGENERATE_GOLDENS=1 cargo test --test plan",
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
    status: pending
  - name: b
    status: pending
    depends-on: [a]
";

    const DUPLICATE_NAME_PLAN: &str = "\
name: demo
changes:
  - name: foo
    status: pending
  - name: foo
    status: pending
";

    const A_DONE_B_PENDING: &str = "\
name: demo
changes:
  - name: a
    status: done
  - name: b
    status: pending
";

    const A_IN_PROGRESS: &str = "\
name: demo
changes:
  - name: a
    status: in-progress
";

    const ALL_DONE: &str = "\
name: demo
changes:
  - name: a
    status: done
  - name: b
    status: done
";

    /// `a` failed, `b` pending depends-on `a`: neither is eligible but
    /// not every entry is terminal, so `next` reports `stuck`.
    const STUCK_PLAN: &str = "\
name: demo
changes:
  - name: a
    status: failed
    status-reason: boom
  - name: b
    status: pending
    depends-on: [a]
";

    const CYCLE_PLAN: &str = "\
name: demo
changes:
  - name: a
    status: pending
    depends-on: [c]
  - name: b
    status: pending
    depends-on: [a]
  - name: c
    status: pending
    depends-on: [b]
";

    const FAILED_WITH_REASON: &str = "\
name: demo
changes:
  - name: a
    status: failed
    status-reason: boom
";

    /// Verbatim RFC-2 §"The Plan" platform-v2 example. Used by the
    /// status smoke test so status output stays pinned to the RFC
    /// reference shape across L1.J–L1.L.
    const PLATFORM_V2_PLAN: &str = r#"name: platform-v2

sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git

changes:
  - name: user-registration
    sources: [monolith]
    status: done

  - name: email-verification
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress

  - name: registration-duplicate-email-crash
    affects: [user-registration]
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending

  - name: notification-preferences
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending

  - name: extract-shared-validation
    affects: [user-registration, email-verification]
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending

  - name: product-catalog
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending

  - name: shopping-cart
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending

  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.

  - name: checkout-ui
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
"#;

    // -- validate ----------------------------------------------------------

    #[test]
    fn plan_validate_clean_plan_text() {
        let project = Project::init();
        project.seed_plan(CLEAN_PLAN);

        let assert =
            specify().current_dir(project.root()).args(["plan", "validate"]).assert().success();
        assert_eq!(assert.get_output().status.code(), Some(0));

        let stdout = std::str::from_utf8(&assert.get_output().stdout).expect("utf8");
        // No ERROR-level lines on a clean plan.
        assert!(
            !stdout.contains("ERROR"),
            "clean plan must not print any ERROR lines, got:\n{stdout}"
        );
    }

    #[test]
    fn plan_validate_clean_plan_json() {
        let project = Project::init();
        project.seed_plan(CLEAN_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "plan", "validate"])
            .assert()
            .success();
        assert_eq!(assert.get_output().status.code(), Some(0));

        let actual = parse_stdout(&assert.get_output().stdout, project.root());
        assert_eq!(actual["schema_version"], 1);
        assert_eq!(actual["passed"], true);
        assert_eq!(actual["results"], Value::Array(vec![]));
        assert_golden("validate-clean.json", actual);
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
        assert_eq!(actual["schema_version"], 1);
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
    fn plan_next_picks_first_pending_text() {
        let project = Project::init();
        project.seed_plan(A_DONE_B_PENDING);

        let assert =
            specify().current_dir(project.root()).args(["plan", "next"]).assert().success();
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
        assert_eq!(actual["schema_version"], 1);
        assert_eq!(actual["next"], "b");
        assert_eq!(actual["reason"], Value::Null);
        assert_eq!(actual["active"], Value::Null);
        assert_golden("next-first-pending.json", actual);
    }

    #[test]
    fn plan_next_reports_in_progress() {
        let project = Project::init();
        project.seed_plan(A_IN_PROGRESS);

        let text = specify().current_dir(project.root()).args(["plan", "next"]).assert().success();
        let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
        assert!(stdout.contains("a"), "text output should mention 'a': {stdout:?}");

        let json = specify()
            .current_dir(project.root())
            .args(["--format", "json", "plan", "next"])
            .assert()
            .success();
        let actual = parse_stdout(&json.get_output().stdout, project.root());
        assert_eq!(actual["schema_version"], 1);
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
        assert_eq!(actual["schema_version"], 1);
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
        assert_eq!(actual["schema_version"], 1);
        assert_eq!(actual["reason"], "stuck");
        assert_eq!(actual["next"], Value::Null);
        assert_eq!(actual["active"], Value::Null);
        assert_golden("next-stuck.json", actual);
    }

    // -- status ------------------------------------------------------------

    #[test]
    fn plan_status_renders_counts_and_topo_order_json() {
        let project = Project::init();
        project.seed_plan(PLATFORM_V2_PLAN);

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "plan", "status"])
            .assert()
            .success();
        let actual = parse_stdout(&assert.get_output().stdout, project.root());

        assert_eq!(actual["schema_version"], 1);
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
    fn plan_status_on_cycle_falls_back_to_list_order() {
        let project = Project::init();
        project.seed_plan(CYCLE_PLAN);

        let output = specify()
            .current_dir(project.root())
            .args(["--format", "json", "plan", "status"])
            .assert()
            .success();

        let actual = parse_stdout(&output.get_output().stdout, project.root());
        assert_eq!(actual["schema_version"], 1);
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
    fn plan_status_surfaces_status_reason_on_failed_entry() {
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
    fn plan_status_missing_plan_file_errors() {
        let project = Project::init();
        // Deliberately do NOT seed plan.yaml.

        let assert = specify()
            .current_dir(project.root())
            .args(["--format", "json", "plan", "status"])
            .assert()
            .failure();
        assert_eq!(assert.get_output().status.code(), Some(1));
        let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
        assert_eq!(value["error"], "config");
        assert!(
            value["message"].as_str().unwrap_or_default().contains("plan file not found"),
            "message should mention 'plan file not found', got: {}",
            value["message"]
        );
    }
}
