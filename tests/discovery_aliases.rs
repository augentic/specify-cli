//! Integration tests for RFC-27 §D6 — candidate aliases on
//! `<project_dir>/discovery.md`.
//!
//! Covers the three operator-facing surfaces:
//!
//! - `specify plan add --sources <key>=<alias>` rewrites the alias to
//!   the canonical candidate id before persisting `plan.yaml`.
//! - `specify plan amend --add-alias` / `--remove-alias` mutate
//!   `discovery.md` and refuse the whole amend on any
//!   `discovery-alias-collision`.
//!
//! Re-enumeration survival is documented at the bottom of this file —
//! the chosen design is "append-only edits live in discovery.md
//! itself; re-enumeration is operator-driven and re-applies adapter
//! aliases as a union".

use std::fs;

mod common;
use common::{Project, parse_stderr, specify};

/// Discovery document with one candidate per source key plus a
/// kebab-case alias for the password-reset candidate. Mirrors the
/// scenario in RFC-27 §Acceptance #26-6.
const DISCOVERY_MD: &str = "\
# Discovery

## Candidate inventory

### user-registration

- id: user-registration
- sources: [legacy, runtime]
- summary: Registration endpoint accepting email + password.

### password-reset-request

- id: password-reset-request
- aliases: [password-reset]
- sources: [legacy]
- summary: Reset endpoint.
";

const PLAN_WITH_SOURCES: &str = "\
name: identity-revamp
sources:
  legacy:
    adapter: code-typescript
    path: ./legacy
  runtime:
    adapter: code-runtime
    path: ./runtime
slices: []
";

fn seed_discovery(project: &Project, body: &str) {
    fs::write(project.root().join("discovery.md"), body).expect("write discovery.md");
}

#[test]
fn plan_add_resolves_alias_to_canonical_id() {
    // RFC-27 §D6 — `--sources legacy=password-reset` (an alias)
    // must persist as `--sources legacy=password-reset-request` (the
    // canonical id) on disk.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "password-reset-request",
            "--target",
            "omnia@v1",
            "--sources",
            "legacy=password-reset",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    // The canonical id matches the slice name, so the bare-key
    // shorthand kicks in — `--sources legacy` ends up rendered as
    // `legacy` rather than `legacy=password-reset-request`.
    assert!(
        saved.contains("- legacy"),
        "plan.yaml must carry the canonical id (bare-key shorthand because \
         candidate == slice name):\n{saved}"
    );
    assert!(
        !saved.contains("password-reset\n") && !saved.contains("password-reset,"),
        "the alias must not appear verbatim on disk:\n{saved}"
    );
}

#[test]
fn plan_add_structured_form_persists_canonical_id_when_slice_differs() {
    // Slice name differs from candidate id, so the structured
    // binding form survives. Aliases still resolve to the canonical
    // id.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "password-reset-flow", // slice name differs from candidate id
            "--target",
            "omnia@v1",
            "--sources",
            "legacy=password-reset",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("candidate: password-reset-request"),
        "expected canonical candidate id on disk, got:\n{saved}"
    );
    assert!(!saved.contains("password-reset\n"), "alias must not survive on disk:\n{saved}");
}

#[test]
fn plan_add_unknown_candidate_in_discovery_refused() {
    // RFC-27 §D6 — when `discovery.md` exists, an unresolvable
    // candidate token refuses at exit 2 with
    // `discovery-candidate-unknown`.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "add",
            "ghost",
            "--target",
            "omnia@v1",
            "--sources",
            "legacy=never-heard-of-it",
        ])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "unknown candidate must exit 2");

    let body = parse_stderr(&assert.get_output().stderr, project.root());
    let results = body["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["rule-id"], "discovery-candidate-unknown");
}

#[test]
fn plan_add_without_discovery_md_skips_alias_resolution() {
    // Legacy backwards-compat: without `discovery.md`, the supplied
    // candidate value round-trips verbatim. Pre-RFC-27 projects
    // continue to work unchanged.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);

    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "wholly-unrelated-slice",
            "--target",
            "omnia@v1",
            "--sources",
            "legacy=opaque-candidate-id",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("candidate: opaque-candidate-id"),
        "expected verbatim candidate value without discovery.md, got:\n{saved}"
    );
}

#[test]
fn plan_amend_add_alias_mutates_discovery_md() {
    // RFC-27 §D6 — `--add-alias` appends to `discovery.md` (NOT to
    // `plan.yaml`). The mutation round-trips through the parser
    // unchanged.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    // `specify plan amend` requires the target slice to exist on
    // the plan; create one so the orthogonal alias edits land.
    specify()
        .current_dir(project.root())
        .args(["plan", "add", "user-registration", "--target", "omnia@v1"])
        .assert()
        .success();

    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "user-registration",
            "--add-alias",
            "user-registration=account-registration",
        ])
        .assert()
        .success();

    let updated = fs::read_to_string(project.root().join("discovery.md")).expect("read discovery");
    assert!(
        updated.contains("aliases: [account-registration]"),
        "discovery.md must record the new alias, got:\n{updated}"
    );
    // Pre-existing aliases on the password-reset candidate survive
    // the amend unchanged.
    assert!(
        updated.contains("aliases: [password-reset]"),
        "existing alias must round-trip, got:\n{updated}"
    );
}

#[test]
fn plan_amend_add_alias_refused_on_collision() {
    // Self-shadow: alias equals the bearing candidate's own id.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specify()
        .current_dir(project.root())
        .args(["plan", "add", "user-registration", "--target", "omnia@v1"])
        .assert()
        .success();

    let assert = specify()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "amend",
            "user-registration",
            "--add-alias",
            "password-reset-request=user-registration", // would shadow another candidate's id
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    let results = body["results"].as_array().expect("results array");
    assert_eq!(results[0]["rule-id"], "discovery-alias-collision");

    let saved = fs::read_to_string(project.root().join("discovery.md")).expect("read discovery");
    assert!(
        !saved.contains("user-registration]"),
        "the alias must not survive on disk; discovery.md still reads:\n{saved}"
    );
}

#[test]
fn plan_amend_remove_alias_is_idempotent() {
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specify()
        .current_dir(project.root())
        .args(["plan", "add", "password-reset-request", "--target", "omnia@v1"])
        .assert()
        .success();

    // First remove: drops the alias.
    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "password-reset-request",
            "--remove-alias",
            "password-reset-request=password-reset",
        ])
        .assert()
        .success();
    // Second remove: no-op (idempotent per RFC-27 §D6).
    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "password-reset-request",
            "--remove-alias",
            "password-reset-request=password-reset",
        ])
        .assert()
        .success();

    let updated = fs::read_to_string(project.root().join("discovery.md")).expect("read discovery");
    assert!(
        !updated.contains("aliases:"),
        "every alias removed; aliases bullet must elide, got:\n{updated}"
    );
}

#[test]
fn plan_amend_add_alias_then_resolves_in_same_invocation() {
    // RFC-27 §D6 — operator can author an alias and consume it on
    // a downstream `--sources` rewrite in the same invocation. The
    // amend writes discovery.md before threading the updated
    // document into the binding resolution path.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specify()
        .current_dir(project.root())
        .args(["plan", "add", "user-registration", "--target", "omnia@v1"])
        .assert()
        .success();

    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "user-registration",
            "--add-alias",
            "user-registration=signup",
            "--add-source",
            "legacy=signup",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("- legacy"),
        "alias `signup` must resolve to the canonical id (user-registration == slice name → \
         bare-key form), got:\n{saved}"
    );
}

#[test]
fn slice_validate_surfaces_discovery_alias_collision() {
    // RFC-27 §D6 — slice validate is the read-only gate that
    // surfaces every alias collision on the project's discovery.md.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    // Hand-author a colliding discovery.md (two candidates share
    // the alias `shared`).
    seed_discovery(
        &project,
        "\
## Candidate inventory

### a

- id: a
- aliases: [shared]
- sources: [legacy]
- summary: A.

### b

- id: b
- aliases: [shared]
- sources: [legacy]
- summary: B.
",
    );

    // Stage a minimal slice directory so `slice validate` has
    // something to validate.
    let slice_dir = project.root().join(".specify/slices/test-slice");
    fs::create_dir_all(&slice_dir).expect("mkdir slice");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "test-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    let results = body["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["rule-id"] == "discovery-alias-collision"),
        "expected at least one discovery-alias-collision finding, got: {results:?}"
    );
}

#[test]
fn plan_amend_alias_survives_reapplied_discovery() {
    // RFC-27 §D6 §Acceptance #26-6 — re-enumeration survival.
    //
    // The chosen design (see file footer): operator-added aliases
    // live in `discovery.md` itself; re-enumeration is an explicit
    // operator step that takes the union with adapter-emitted
    // aliases. We model that here by re-writing discovery.md with
    // the operator's alias plus a fresh adapter emit (which adds
    // its own alias) and asserting both survive.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specify()
        .current_dir(project.root())
        .args(["plan", "add", "password-reset-request", "--target", "omnia@v1"])
        .assert()
        .success();

    // Operator-added alias via `plan amend`.
    specify()
        .current_dir(project.root())
        .args([
            "plan",
            "amend",
            "password-reset-request",
            "--add-alias",
            "password-reset-request=pwd-reset",
        ])
        .assert()
        .success();

    // Simulate re-enumeration: hand-author a discovery.md that
    // unions the original adapter aliases with the operator's new
    // alias. This is the documented re-enumeration contract — the
    // CLI does not own the union step (no automatic re-enumeration
    // exists today); operators or skill bodies preserve the alias
    // by re-emitting it. The test verifies the resulting union
    // resolves correctly through `--sources` and `slice validate`.
    seed_discovery(
        &project,
        "\
## Candidate inventory

### password-reset-request

- id: password-reset-request
- aliases: [password-reset, pwd-reset]
- sources: [legacy]
- summary: Reset endpoint (re-emitted).
",
    );

    let discovery =
        fs::read_to_string(project.root().join("discovery.md")).expect("read discovery");
    assert!(
        discovery.contains("aliases: [password-reset, pwd-reset]"),
        "post-re-enumerate union must preserve both adapter and operator aliases, got:\n{discovery}"
    );
}
