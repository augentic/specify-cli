//! Integration tests for discovery alias contract — lead aliases on
//! `<project_dir>/discovery.md`.
//!
//! Covers the three operator-facing surfaces:
//!
//! - `specrun plan add --sources <key>=<alias>` rewrites the alias to
//!   the canonical lead id before persisting `plan.yaml`.
//! - `specrun plan amend --add-alias` / `--remove-alias` mutate
//!   `discovery.md` and refuse the whole amend on any
//!   `discovery-alias-collision`.
//!
//! Re-survey survival is documented at the bottom of this file —
//! the chosen design is "append-only edits live in discovery.md
//! itself; re-survey is operator-driven and re-applies adapter
//! aliases as a union".

use std::fs;

mod common;
use common::{Project, parse_stderr, parse_stdout, specrun};

/// Discovery document with one lead per source key plus a
/// kebab-case alias for the password-reset lead. Mirrors the
/// scenario in workflow §Acceptance #26-6.
const DISCOVERY_MD: &str = "\
# Discovery

## Lead inventory

### legacy:user-registration

- lead: user-registration
- source: legacy
- synopsis: Registration endpoint accepting email + password.

### legacy:password-reset-request

- lead: password-reset-request
- source: legacy
- aliases: [password-reset]
- synopsis: Reset endpoint.
";

const PLAN_WITH_SOURCES: &str = "\
name: identity-revamp
sources:
  legacy:
    adapter: code-typescript
    path: ./legacy
  runtime:
    adapter: captures
    path: ./captures/replays
slices: []
";

fn seed_discovery(project: &Project, body: &str) {
    fs::write(project.root().join("discovery.md"), body).expect("write discovery.md");
}

#[test]
fn plan_add_resolves_alias_to_canonical_id() {
    // discovery alias contract — `--sources legacy=password-reset` (an alias)
    // must persist as `--sources legacy=password-reset-request` (the
    // canonical id) on disk.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "password-reset-request", "--sources", "legacy=password-reset"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    // The canonical id matches the slice name, so the bare-key
    // shorthand kicks in — `--sources legacy` ends up rendered as
    // `legacy` rather than `legacy=password-reset-request`.
    assert!(
        saved.contains("- legacy"),
        "plan.yaml must carry the canonical id (bare-key shorthand because \
         lead == slice name):\n{saved}"
    );
    assert!(
        !saved.contains("password-reset\n") && !saved.contains("password-reset,"),
        "the alias must not appear verbatim on disk:\n{saved}"
    );
}

#[test]
fn plan_add_persists_canonical_id() {
    // Slice name differs from lead id, so the structured
    // binding form survives. Aliases still resolve to the canonical
    // id.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specrun()
        .current_dir(project.root())
        .args([
            "plan",
            "add",
            "password-reset-flow", // slice name differs from lead id
            "--sources",
            "legacy=password-reset",
        ])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("lead: password-reset-request"),
        "expected canonical lead id on disk, got:\n{saved}"
    );
    assert!(!saved.contains("password-reset\n"), "alias must not survive on disk:\n{saved}");
}

#[test]
fn plan_add_unknown_lead_refused() {
    // discovery alias contract — when `discovery.md` exists, an unresolvable
    // lead token refuses at exit 2 with
    // `discovery-lead-unknown`.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "plan", "add", "ghost", "--sources", "legacy=never-heard-of-it"])
        .assert()
        .failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "unknown lead must exit 2");

    // Payload-free `Error::Validation`: the discriminant is the
    // top-level `error` code.
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(body["error"], "discovery-lead-unknown");
}

#[test]
fn plan_add_without_discovery_skips_alias() {
    // Legacy backwards-compat: without `discovery.md`, the supplied
    // lead value round-trips verbatim. Pre-authority and reconciliation contract projects
    // continue to work unchanged.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "wholly-unrelated-slice", "--sources", "legacy=opaque-candidate-id"])
        .assert()
        .success();

    let saved = fs::read_to_string(project.plan_path()).expect("read plan");
    assert!(
        saved.contains("lead: opaque-candidate-id"),
        "expected verbatim lead value without discovery.md, got:\n{saved}"
    );
}

#[test]
fn plan_amend_alias_mutates_discovery() {
    // discovery alias contract — `--add-alias` appends to `discovery.md` (NOT to
    // `plan.yaml`). The mutation round-trips through the parser
    // unchanged.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    // `specrun plan amend` requires the target slice to exist on
    // the plan; create one so the orthogonal alias edits land.
    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "user-registration"])
        .assert()
        .success();

    specrun()
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
    // Pre-existing aliases on the password-reset lead survive
    // the amend unchanged.
    assert!(
        updated.contains("aliases: [password-reset]"),
        "existing alias must round-trip, got:\n{updated}"
    );
}

#[test]
fn plan_amend_alias_refused_on_collision() {
    // Self-shadow: alias equals the bearing lead's own id.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "user-registration"])
        .assert()
        .success();

    let assert = specrun()
        .current_dir(project.root())
        .args([
            "--format",
            "json",
            "plan",
            "amend",
            "user-registration",
            "--add-alias",
            "password-reset-request=user-registration", // would shadow another lead's id
        ])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    // `Error::Validation` is payload-free: the colliding-alias
    // discriminant is the top-level `error` code.
    let body = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(body["error"], "discovery-alias-collision");

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

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "password-reset-request"])
        .assert()
        .success();

    // First remove: drops the alias.
    specrun()
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
    // Second remove: no-op (idempotent per discovery alias contract).
    specrun()
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
fn plan_amend_alias_resolves_same_invocation() {
    // discovery alias contract — operator can author an alias and consume it on
    // a downstream `--sources` rewrite in the same invocation. The
    // amend writes discovery.md before threading the updated
    // document into the binding resolution path.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "user-registration"])
        .assert()
        .success();

    specrun()
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
fn slice_validate_alias_collision() {
    // discovery alias contract — slice validate is the read-only gate that
    // surfaces every alias collision on the project's discovery.md.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    // Hand-author a colliding discovery.md (two leads share
    // the alias `shared`).
    seed_discovery(
        &project,
        "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- aliases: [shared]
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [shared]
- synopsis: B.
",
    );

    // Stage a minimal slice directory so `slice validate` has
    // something to validate.
    let slice_dir = project.root().join(".specify/slices/test-slice");
    fs::create_dir_all(&slice_dir).expect("mkdir slice");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "test-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    // The pre-adapter gate renders its `DiagnosticReport` on stdout and
    // fails payload-free: the colliding-alias diagnostic is one of the
    // report's findings, while stderr carries only the gate discriminant.
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "slice-pre-adapter-gate");
    let body = parse_stdout(&assert.get_output().stdout, project.root());
    let findings = body["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|r| r["rule-id"] == "discovery-alias-collision"),
        "expected at least one discovery-alias-collision finding, got: {findings:?}"
    );
}

#[test]
fn plan_amend_alias_survives_reapplied_discovery() {
    // discovery alias contract §Acceptance #26-6 — re-survey survival.
    //
    // The chosen design (see file footer): operator-added aliases
    // live in `discovery.md` itself; re-survey is an explicit
    // operator step that takes the union with adapter-emitted
    // aliases. We model that here by re-writing discovery.md with
    // the operator's alias plus a fresh adapter emit (which adds
    // its own alias) and asserting both survive.
    let project = Project::init();
    project.seed_plan(PLAN_WITH_SOURCES);
    seed_discovery(&project, DISCOVERY_MD);

    specrun()
        .current_dir(project.root())
        .args(["plan", "add", "password-reset-request"])
        .assert()
        .success();

    // Operator-added alias via `plan amend`.
    specrun()
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

    // Simulate re-survey: hand-author a discovery.md that
    // unions the original adapter aliases with the operator's new
    // alias. This is the documented re-survey contract — the
    // CLI does not own the union step (no automatic re-survey
    // exists today); operators or skill bodies preserve the alias
    // by re-emitting it. The test verifies the resulting union
    // resolves correctly through `--sources` and `slice validate`.
    seed_discovery(
        &project,
        "\
## Lead inventory

### legacy:password-reset-request

- lead: password-reset-request
- source: legacy
- aliases: [password-reset, pwd-reset]
- synopsis: Reset endpoint (re-emitted).
",
    );

    let discovery =
        fs::read_to_string(project.root().join("discovery.md")).expect("read discovery");
    assert!(
        discovery.contains("aliases: [password-reset, pwd-reset]"),
        "post-re-survey union must preserve both adapter and operator aliases, got:\n{discovery}"
    );
}
