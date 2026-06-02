//! Plan archive CLI tests, including the working-directory co-move.

use crate::support::*;

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
             \x20\x20\x20\x20project: alpha\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyclic-b]\n\
             \x20\x20- name: cyclic-b\n\
             \x20\x20\x20\x20project: alpha\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyclic-a]\n\
             \x20\x20- name: orphaned-source-user\n\
             \x20\x20\x20\x20project: alpha\n\
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
fn validate_reports_topology_cache_stale() {
    // RFC-36: a slot's `project.yaml` is the authored home for its
    // facets; `.specify/topology.lock` is the derived projection. When a
    // materialised slot drifts from the committed cache, `plan validate`
    // emits the warning-only `topology-cache-stale` diagnostic whose fix
    // is `specrun workspace sync`. (Replaces the former
    // `adapter-mismatch-workspace` check.)
    let tmp = tempdir().unwrap();
    init_omnia_project(&tmp);

    fs::write(
        tmp.path().join("plan.yaml"),
        "name: demo\n\
             slices:\n\
             \x20\x20- name: alpha-slice\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20project: alpha\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join("registry.yaml"),
        "version: 1\n\
             projects:\n\
             \x20\x20- name: alpha\n\
             \x20\x20\x20\x20url: git@github.com:org/alpha.git\n",
    )
    .unwrap();

    // Materialise the slot with a resolvable adapter and an authored
    // description, then seed a topology.lock whose entry disagrees.
    let slot_specify = tmp.path().join(".specify/workspace/alpha/.specify");
    fs::create_dir_all(&slot_specify).unwrap();
    fs::write(
        slot_specify.join("project.yaml"),
        "name: alpha\nadapter: omnia@v1\ndescription: Fresh description\n",
    )
    .unwrap();
    copy_dir(&omnia_schema_dir(), &slot_specify.join(".cache/manifests/targets/omnia"));

    fs::write(
        tmp.path().join(".specify/topology.lock"),
        "version: 1\n\
             projects:\n\
             \x20\x20- name: alpha\n\
             \x20\x20\x20\x20target: omnia@v1\n\
             \x20\x20\x20\x20description: Stale description\n",
    )
    .unwrap();

    let assert =
        specrun().current_dir(tmp.path()).args(["--format", "json", "plan", "validate"]).assert();
    let value: Value =
        serde_json::from_str(&String::from_utf8(assert.get_output().stdout.clone()).expect("utf8"))
            .expect("stdout is JSON");
    let results = value["results"].as_array().expect("results array");
    let stale: Vec<&Value> =
        results.iter().filter(|r| r["code"] == "topology-cache-stale").collect();
    assert_eq!(stale.len(), 1, "expected one topology-cache-stale finding, got: {results:#?}");
    assert_eq!(stale[0]["severity"], "warning");
    let msg = stale[0]["message"].as_str().expect("message string");
    assert!(msg.contains("alpha"), "expected slot name in message, got: {msg}");
    assert!(msg.contains("workspace sync"), "expected the fix command in message, got: {msg}");
    assert_eq!(value["passed"], true, "stale cache is warning-only");
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
             \x20\x20\x20\x20project: default\n\
             \x20\x20\x20\x20status: pending\n\
             \x20\x20\x20\x20depends-on: [cyc-b]\n\
             \x20\x20- name: cyc-b\n\
             \x20\x20\x20\x20project: default\n\
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
        .args(["--format", "json", "plan", "add", "add-search-filter", "--sources", "intent"])
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
        .args(["--format", "json", "plan", "add", "foo", "--sources", "intent=different-candidate"])
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
        .args(["--format", "json", "plan", "add", "foo", "--sources", "intent="])
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
        .args(["plan", "add", "foo", "--sources", "intent"])
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
        .args(["plan", "add", "foo", "--sources", "intent"])
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

    specrun().current_dir(project.root()).args(["plan", "add", "foo"]).assert().success();

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

    specrun().current_dir(project.root()).args(["plan", "add", "foo"]).assert().success();

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

    specrun().current_dir(project.root()).args(["plan", "add", "foo"]).assert().success();

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

    specrun().current_dir(project.root()).args(["plan", "add", "foo"]).assert().success();

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

