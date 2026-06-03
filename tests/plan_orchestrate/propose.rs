//! `specify plan propose` CLI tests: dry-run request envelopes,
//! `--from` happy paths, journal tail, negative gates, and re-propose
//! semantics, plus the propose-only seeds and helpers.

use crate::support::*;

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
/// project) and carries the explicit slice `name`.
const PROPOSE_RESPONSE_N1: &str = r#"{
  "version": 1,
  "kind": "response",
  "slices": [
    { "name": "fix-typo", "sources": [{ "source": "intent", "lead": "fix-typo" }] }
  ]
}"#;

/// Workspace registry with two projects bound to different target adapters —
/// the topology the fan-out response binds against.
const PROPOSE_REGISTRY_WORKSPACE: &str = "\
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

/// Workspace surveyed inventory: four leads across `docs` + `legacy` (the
/// proposal-schema envelope example, in document order).
const PROPOSE_DISCOVERY_WORKSPACE: &str = "\
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

/// Committed `.specify/topology.lock` for the workspace fixture (RFC-36) —
/// the projection `workspace sync` would derive from each member
/// project's `project.yaml`. Descriptions mirror the registry seeds so
/// the request envelope's `projects[]` stays the authoritative shape.
const PROPOSE_TOPOLOGY_WORKSPACE: &str = "\
version: 1
projects:
  - name: identity-contracts
    target: contracts@v1
    description: Versioned API contracts crate for the identity domain.
  - name: identity-service
    target: omnia@v1
    description: Omnia identity service implementing auth and password flows.
";

/// Workspace plan declaring the two surveyed source keys, no slices yet.
const PROPOSE_PLAN_WORKSPACE: &str = "\
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
/// example): the `identity-api` lead is referenced by two slices
/// (`identity-contracts` + `identity-service`, joined by `depends-on`);
/// `password-reset` is a single slice matched across sources by summary.
const PROPOSE_RESPONSE_FANOUT: &str = r#"{
  "version": 1,
  "kind": "response",
    "slices": [
    {
      "name": "identity-contracts",
      "sources": [
        { "source": "docs", "lead": "identity-api" },
        { "source": "legacy", "lead": "identity-api" }
      ],
      "project": "identity-contracts",
      "rationale": "identity API surface matched by shared slug across docs + legacy"
    },
    {
      "name": "identity-service",
      "sources": [
        { "source": "docs", "lead": "identity-api" },
        { "source": "legacy", "lead": "identity-api" }
      ],
      "project": "identity-service",
      "depends-on": ["identity-contracts"]
    },
    {
      "name": "password-reset",
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

/// Scaffold a workspace project in a fresh tempdir, seeding
/// `registry.yaml`, `discovery.md`, and `plan.yaml`.
fn workspace_project(registry: &str, discovery: &str, plan: &str) -> TempDir {
    let tmp = tempdir().expect("tempdir");
    init_workspace(&tmp, "platform-workspace");
    fs::write(tmp.path().join("registry.yaml"), registry).expect("write registry.yaml");
    seed_discovery(tmp.path(), discovery);
    fs::write(tmp.path().join("plan.yaml"), plan).expect("write plan.yaml");
    // RFC-36: workspace plan-time topology reads the committed cache, not the
    // registry. Seed the projection `workspace sync` would produce for
    // the remote members (which a unit test cannot materialise).
    fs::write(tmp.path().join(".specify/topology.lock"), PROPOSE_TOPOLOGY_WORKSPACE)
        .expect("write topology.lock");
    tmp
}

/// Run `plan propose --from <body>` expecting an exit-2 abort and
/// return the parsed `--format json` stderr envelope.
fn propose_from_stderr(root: &Path, body: &str) -> Value {
    let response = write_response(root, body);
    let assert = specify_cmd()
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
    let assert = specify_cmd()
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

    let assert = specify_cmd()
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
fn propose_dry_run_workspace_request_golden() {
    // Workspace: the registry's two projects and four leads across two
    // sources project verbatim into the request envelope.
    let tmp = workspace_project(
        PROPOSE_REGISTRY_WORKSPACE,
        PROPOSE_DISCOVERY_WORKSPACE,
        PROPOSE_PLAN_WORKSPACE,
    );

    let assert = specify_cmd()
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

    assert_golden("propose-dry-run-workspace-request.json", actual);
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
    assert_eq!(actual["slice-names"], serde_json::json!(["fix-typo"]));
    assert_golden("propose-from-n1-summary.json", actual);

    // The projected plan: one slice, target derived from the
    // auto-bound project, structured source binding.
    let plan = Plan::load(&project.plan_path()).expect("load plan");
    assert_eq!(plan.entries.len(), 1);
    let entry = &plan.entries[0];
    assert_eq!(entry.name, "fix-typo");
    // Target is no longer stored on the slice; the bound project is the
    // sole binding and the target resolves from it on demand.
    assert_eq!(entry.project.as_deref(), Some("test-proj"));
    assert_eq!(entry.sources.len(), 1);
    assert_eq!(entry.sources[0].source(), "intent");
    assert_eq!(entry.sources[0].lead("fix-typo"), "fix-typo");
}

#[test]
fn propose_from_fan_out_golden() {
    let tmp = workspace_project(
        PROPOSE_REGISTRY_WORKSPACE,
        PROPOSE_DISCOVERY_WORKSPACE,
        PROPOSE_PLAN_WORKSPACE,
    );

    let actual = propose_from_ok(tmp.path(), PROPOSE_RESPONSE_FANOUT);
    assert_eq!(actual["plan"]["name"], "identity-revamp");
    assert_eq!(actual["slice-count"], 3);
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

    // Targets are no longer stored on slices; the bound projects above
    // are the sole bindings and resolve to their adapters on demand.

    // The fan-out slice carries both matched sources, structured.
    assert_eq!(plan.entries[0].sources.len(), 2);
    assert_eq!(plan.entries[0].sources[0].source(), "docs");
    assert_eq!(plan.entries[0].sources[1].source(), "legacy");
    // The password-reset slice keeps the cross-source leads verbatim.
    assert_eq!(plan.entries[2].sources[1].source(), "legacy");
    assert_eq!(plan.entries[2].sources[1].lead("password-reset"), "reset-password");
    // depends-on resolves to a derived slice name.
    assert_eq!(plan.entries[1].depends_on, ["identity-contracts"]);
    assert!(plan.entries[0].depends_on.is_empty());
}

// -- journal tail -----------------------------------------------------

#[test]
fn propose_from_emits_single_journal_tail() {
    let tmp = workspace_project(
        PROPOSE_REGISTRY_WORKSPACE,
        PROPOSE_DISCOVERY_WORKSPACE,
        PROPOSE_PLAN_WORKSPACE,
    );
    let response = write_response(tmp.path(), PROPOSE_RESPONSE_FANOUT);
    specify_cmd()
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
    assert_eq!(events.len(), 1, "exactly one reconcile event fires, got:\n{events:#?}");

    // RFC-29 review F8 folded the former agent/completed pair into one
    // `plan.reconcile.completed` event carrying the slice names in order.
    let completed = &events[0];
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

    let assert = specify_cmd()
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

    let assert = specify_cmd()
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
        r#"{"version":1,"slices":[{"name":"a","sources":[{"source":"docs","lead":"a"}]}]}"#,
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
        r#"{"version":1,"kind":"response","slices":[{"name":"s","sources":[{"source":"docs","lead":"ghost"}]}]}"#,
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
        r#"{"version":1,"kind":"response","slices":[{"name":"s","sources":[{"source":"docs","lead":"a"},{"source":"docs","lead":"b"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-slice-source-collision");
}

#[test]
fn propose_reconcile_partition_gap() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // The catalog carries two leads; the response covers only `a`.
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"s","sources":[{"source":"docs","lead":"a"}]}]}"#,
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
        r#"{"version":1,"kind":"response","slices":[{"name":"s","sources":[{"source":"docs","lead":"a"}],"project":"ghost"}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-project-orphan");
}

#[test]
fn reconcile_project_binding_required() {
    // Two projects offered (workspace); the slice omits `project`, so the
    // kernel cannot auto-bind.
    let tmp = workspace_project(
        PROPOSE_REGISTRY_WORKSPACE,
        &discovery_doc(&[("docs", "a")]),
        "name: identity-revamp\nslices: []\n",
    );

    let body = propose_from_stderr(
        tmp.path(),
        r#"{"version":1,"kind":"response","slices":[{"name":"s","sources":[{"source":"docs","lead":"a"}]}]}"#,
    );
    assert_eq!(body["error"], "plan-reconcile-project-binding-required");
}

#[test]
fn propose_reconcile_slice_name_collision() {
    let project = Project::init();
    project.seed_plan("name: demo\nslices: []\n");
    seed_discovery(project.root(), &discovery_doc(&[("docs", "a"), ("docs", "b")]));

    // Two distinct slices both supply the name `dup`. Name uniqueness is
    // the sole duplicate gate now that `scope` is gone (RFC-29 review F3).
    let body = propose_from_stderr(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"dup","sources":[{"source":"docs","lead":"a"}]},{"name":"dup","sources":[{"source":"docs","lead":"b"}]}]}"#,
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
        r#"{"version":1,"kind":"response","slices":[{"name":"alpha","sources":[{"source":"docs","lead":"a"}],"depends-on":["beta"]},{"name":"beta","sources":[{"source":"docs","lead":"b"}],"depends-on":["alpha"]}]}"#,
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

    let assert = specify_cmd()
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
        r#"{"version":1,"kind":"response","slices":[{"name":"first","sources":[{"source":"intent","lead":"fix-typo"}]}]}"#,
    );
    let plan_after_first = Plan::load(&project.plan_path()).expect("load plan");
    assert_eq!(
        plan_after_first.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["first"]
    );

    propose_from_ok(
        project.root(),
        r#"{"version":1,"kind":"response","slices":[{"name":"second","sources":[{"source":"intent","lead":"fix-typo"}]}]}"#,
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
    specify_cmd()
        .current_dir(project.root())
        .args(["plan", "transition", "demo", "approved"])
        .assert()
        .success();

    let body = propose_from_stderr(project.root(), PROPOSE_RESPONSE_N1);
    assert_eq!(body["error"], "plan-reconcile-plan-not-replaceable");
}
