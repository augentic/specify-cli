//! RFC-29 acceptance proof (RFC-29d §"Acceptance proof (D7)").
//!
//! This is the end-to-end fixture that proves fan-in *twice* (Lead sets
//! at `survey`, Evidence at `extract`) and fan-out *once* (multiple
//! single-target slices reconciled from shared source claims), then
//! drives those slices all the way through `build` and `merge` under a
//! `depends-on` ordering and asserts the kernel-projection determinism
//! property.
//!
//! ```text
//! documentation + code-typescript (sources: docs, legacy)
//!   -> source survey            # fan-in #1: Lead sets (incl. docs:password-reset / legacy:reset-password mismatch)
//!   -> plan propose --dry-run   # flat lead catalog + identity-contracts->contracts@v1 / identity-service->omnia@v1
//!   -> plan propose --from      # agent groups leads; kernel writes single-target slices + project bindings + depends-on
//!   -> per slice: source extract -> slice synthesize -> slice build -> slice merge
//!   -> depends-on ordering: identity-contracts merges before identity-service starts
//! ```
//!
//! ## Topology choice (documented simplification)
//!
//! RFC-29d describes the *same-tree registry-symlink* topology, where
//! two registry projects resolve into one working tree via `registry.yaml`
//! URLs materialised as symlinks. Per the C10 pragmatism guidance, this
//! test uses the **workspace + committed `topology.lock`** projection that the
//! shipped `plan propose` tests already exercise (see
//! `tests/workflow/propose.rs::propose_*`) — it exposes the same two
//! projects to `propose` without the symlink-materialisation machinery,
//! which the deterministic proof does not need. Both slices live in one
//! `.specify/slices/` tree and merge into one baseline (`.specify/specs/`),
//! so "two single-target slices sharing one baseline tree, ordered by
//! depends-on" is proven directly. Each slice's bound target is set via
//! `slice create --target <t>` (the CLI surface that stores the bound
//! adapter on `.metadata.yaml`); `slice build` resolves it from there,
//! exactly as in production.
//!
//! ## Coverage delegated to existing tests (not re-implemented here)
//!
//! The exhaustive malformed-`--from` reconcile codes
//! (`plan-reconcile-partition`, `-lead-orphan`,
//! `-slice-source-collision`, `-slice-name-collision`,
//! `-depends-on-cycle`, `-project-binding-required`, `-project-orphan`,
//! `-plan-not-replaceable`) are covered over this exact identity fan-out
//! shape in `tests/workflow/`. The synthesis-kernel
//! normalize-not-reject and per-source orphan/kind-mismatch aborts are
//! covered in `tests/slice.rs::synthesize_normalizes_pre_assigned_fields`.
//! This test asserts the *composed* path and the fan-out-specific guards
//! (`plan-propose-mode-required` plus a `project-binding-required`
//! spot-check on this workspace), then the build / merge / ordering /
//! determinism behaviour no existing test covers.
//!
//! `change.md` rendering of cross-source matches is **agent-owned**: the
//! response `rationale` field is kernel-ignored (see
//! `change/plan/core/propose.rs`), so `change.md` is authored by the
//! `/spec:plan` skill, not the deterministic CLI this test drives. We
//! therefore assert the kernel-side effects of the fan-out (slice
//! bindings, depends-on, the `plan.reconcile.completed` event) rather
//! than the skill-authored `change.md`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use crate::common::{
    copy_dir, init_workspace, omnia_schema_dir, parse_json, parse_stderr, parse_stdout, repo_root,
    specify_cmd,
};

// ---------------------------------------------------------------------------
// Fixture + seed material
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    repo_root().join("tests/fixtures/rfc-29/fan-in-fan-out")
}

fn fixture(rel: &str) -> String {
    let path = fixture_dir().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read fixture {}: {err}", path.display()))
}

/// Hub registry: the two projects bound to different target adapters
/// that the fan-out response binds against.
const REGISTRY_HUB: &str = "\
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

/// Committed plan-time topology projection (RFC-36) the workspace reads in
/// place of materialising the remote members.
const TOPOLOGY_HUB: &str = "\
version: 1
projects:
  - name: identity-contracts
    target: contracts@v1
    description: Versioned API contracts crate for the identity domain.
  - name: identity-service
    target: omnia@v1
    description: Omnia identity service implementing auth and password flows.
";

/// Hub plan declaring the two surveyed sources, no slices yet.
const PLAN_HUB: &str = "\
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

const CONTRACTS_ADAPTER: &str = "\
name: contracts
version: 1
axis: target
execution: agent
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
inputs:
  - path: contracts
    required: true
description: Versioned API contracts target.
";

// ---------------------------------------------------------------------------
// Staging helpers
// ---------------------------------------------------------------------------

/// Author a minimal `execution: agent` source adapter with the two
/// briefs the survey/extract fingerprints hash.
fn stage_source_adapter(root: &Path, name: &str, description: &str) {
    let dir = root.join(format!("adapters/sources/{name}"));
    fs::create_dir_all(dir.join("briefs")).expect("mkdir source adapter briefs");
    fs::write(
        dir.join("adapter.yaml"),
        format!(
            "name: {name}\nversion: 1\naxis: source\nexecution: agent\nbriefs:\n  survey: \
             briefs/survey.md\n  extract: briefs/extract.md\ndescription: {description}\n"
        ),
    )
    .expect("write source adapter.yaml");
    fs::write(dir.join("briefs/survey.md"), "# survey brief\n").expect("write survey brief");
    fs::write(dir.join("briefs/extract.md"), "# extract brief\n").expect("write extract brief");
}

/// Author the `contracts` target adapter (declares a required
/// `contracts` build input) and copy the in-repo `omnia` target.
fn stage_target_adapters(root: &Path) {
    copy_dir(&omnia_schema_dir(), &root.join("adapters/targets/omnia"));
    let dir = root.join("adapters/targets/contracts");
    fs::create_dir_all(dir.join("briefs")).expect("mkdir contracts adapter briefs");
    fs::write(dir.join("adapter.yaml"), CONTRACTS_ADAPTER).expect("write contracts adapter.yaml");
    for op in ["shape", "build", "merge"] {
        fs::write(dir.join(format!("briefs/{op}.md")), format!("# {op} brief\n"))
            .expect("write contracts brief");
    }
}

/// Stand in for the survey agent: drop the golden lead-set into scratch
/// and run `source survey <source> --phase finalize`.
fn survey_finalize(root: &Path, source: &str, adapter: &str, lead_set: &str) {
    let scratch = root.join(format!(".specify/.cache/extractions/{adapter}/survey/scratch"));
    fs::create_dir_all(&scratch).expect("mkdir survey scratch");
    fs::write(scratch.join("lead-set.md"), lead_set).expect("write lead-set.md");
    specify_cmd()
        .current_dir(root)
        .args(["source", "survey", source, "--phase", "finalize"])
        .assert()
        .success();
}

/// Stand in for the extract agent: drop the golden Evidence into scratch
/// and run `source extract <source> <lead> --slice <slice> --phase finalize`.
fn extract_finalize(
    root: &Path, source: &str, adapter: &str, lead: &str, slice: &str, evidence: &str,
) {
    let scratch = root.join(format!(".specify/.cache/extractions/{adapter}/{slice}/scratch"));
    fs::create_dir_all(&scratch).expect("mkdir extract scratch");
    fs::write(scratch.join("evidence.yaml"), evidence).expect("write evidence.yaml");
    specify_cmd()
        .current_dir(root)
        .args(["source", "extract", source, lead, "--slice", slice, "--phase", "finalize"])
        .assert()
        .success();
}

fn journal_lines(root: &Path) -> Vec<String> {
    let path = root.join(".specify/journal.jsonl");
    fs::read_to_string(&path)
        .map(|raw| raw.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn journal_has(root: &Path, event: &str) -> bool {
    journal_lines(root).iter().any(|l| l.contains(&format!(r#""event":"{event}""#)))
}

fn read_plan(root: &Path) -> String {
    fs::read_to_string(root.join("plan.yaml")).expect("read plan.yaml")
}

// ---------------------------------------------------------------------------
// Scenario setup
// ---------------------------------------------------------------------------

/// Stand up the workspace, stage adapters + sources, and run both
/// surveys so `discovery.md` carries all four leads.
fn scenario() -> TempDir {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path();
    init_workspace(&tmp, "identity-revamp");
    fs::write(root.join("registry.yaml"), REGISTRY_HUB).expect("write registry.yaml");
    fs::write(root.join(".specify/topology.lock"), TOPOLOGY_HUB).expect("write topology.lock");
    fs::write(root.join("plan.yaml"), PLAN_HUB).expect("write plan.yaml");

    stage_source_adapter(root, "documentation", "Operator-provided written intent.");
    stage_source_adapter(
        root,
        "code-typescript",
        "Behavioural evidence from a TypeScript codebase.",
    );
    stage_target_adapters(root);

    // The survey/extract fingerprints canonicalise the bound source
    // paths, so both must exist on disk.
    for src in ["docs", "legacy"] {
        fs::create_dir_all(root.join(src)).expect("mkdir bound source dir");
        fs::write(root.join(src).join(".keep"), "").expect("seed bound source dir");
    }

    // Fan-in #1: survey both sources into one discovery.md.
    survey_finalize(root, "docs", "documentation", &fixture("leads/docs.md"));
    survey_finalize(root, "legacy", "code-typescript", &fixture("leads/legacy.md"));
    tmp
}

// ---------------------------------------------------------------------------
// The acceptance proof
// ---------------------------------------------------------------------------

/// The plan-time half of the proof: survey leads (with the deliberate
/// slug mismatch), the `--dry-run` request envelope, the
/// `plan-propose-mode-required` / `project-binding-required` guards, and
/// the `--from` fan-out that writes single-target slices with project
/// bindings + depends-on and emits `plan.reconcile.completed`.
fn prove_plan_time_fan_out(root: &Path) {
    // Survey produced schema-valid leads, including the deliberate
    // docs:password-reset / legacy:reset-password slug mismatch.
    let discovery = fs::read_to_string(root.join("discovery.md")).expect("read discovery.md");
    for block in [
        "### docs:identity-api",
        "### docs:password-reset",
        "### legacy:identity-api",
        "### legacy:reset-password",
    ] {
        assert!(discovery.contains(block), "discovery.md missing {block}, got:\n{discovery}");
    }

    // `propose --dry-run` returns a kind:request envelope exposing both
    // projects and one lead row per (source, lead), and writes nothing.
    let plan_before = read_plan(root);
    let dry = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "plan", "propose", "--dry-run"])
        .assert()
        .success();
    let request = parse_stdout(&dry.get_output().stdout, root);
    assert_eq!(request["kind"], "request");
    let projects = request["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0]["name"], "identity-contracts");
    assert_eq!(projects[0]["target"], "contracts@v1");
    assert_eq!(projects[1]["name"], "identity-service");
    assert_eq!(projects[1]["target"], "omnia@v1");
    let leads = request["leads"].as_array().expect("leads array");
    assert_eq!(leads.len(), 4, "one row per (source, lead): {leads:#?}");
    assert_eq!(read_plan(root), plan_before, "--dry-run must not touch plan.yaml");

    // Neither flag aborts mode-required.
    let no_mode = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "plan", "propose"])
        .assert()
        .failure();
    assert_eq!(no_mode.get_output().status.code(), Some(2));
    assert_eq!(
        parse_stderr(&no_mode.get_output().stderr, root)["error"],
        "plan-propose-mode-required"
    );

    // Fan-out-specific reconcile guard: with two projects offered, a slice
    // that covers its leads cleanly but omits `project` aborts
    // project-binding-required. (The full partition is satisfied so this
    // is not a partition/collision abort.) The remaining reconcile codes
    // are covered exhaustively over this shape in tests/workflow/.
    fs::write(
        root.join("bad-response.json"),
        r#"{"version":1,"kind":"response","slices":[{"name":"unbound","sources":[{"source":"docs","lead":"identity-api"},{"source":"legacy","lead":"identity-api"}]},{"name":"reset","project":"identity-service","sources":[{"source":"docs","lead":"password-reset"},{"source":"legacy","lead":"reset-password"}]}]}"#,
    )
    .expect("write bad response");
    let bound = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "plan", "propose", "--from", "bad-response.json"])
        .assert()
        .failure();
    assert_eq!(
        parse_stderr(&bound.get_output().stderr, root)["error"],
        "plan-reconcile-project-binding-required"
    );

    // `propose --from` writes single-target slices with project bindings +
    // depends-on and emits plan.reconcile.completed.
    fs::write(root.join("response.json"), fixture("propose-response.json"))
        .expect("write response.json");
    let from = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "plan", "propose", "--from", "response.json"])
        .assert()
        .success();
    let summary = parse_stdout(&from.get_output().stdout, root);
    assert_eq!(summary["slice-count"], 3);
    assert_eq!(
        summary["slice-names"],
        serde_json::json!(["identity-contracts", "identity-service", "password-reset"])
    );
    assert!(journal_has(root, "plan.reconcile.completed"), "fan-out must emit reconcile.completed");

    let plan = read_plan(root);
    assert!(plan.contains("project: identity-contracts"), "contracts slice binds its project");
    assert!(plan.contains("project: identity-service"), "service slice binds its project");
    assert!(plan.contains("depends-on:"), "service depends-on contracts");
    // The cross-source slug mismatch is matched into the third slice.
    assert!(plan.contains("name: password-reset"), "password-reset slice present");
    assert!(plan.contains("lead: reset-password"), "legacy reset-password lead carried verbatim");
}

#[test]
fn fan_in_twice_fan_out_once() {
    let tmp = scenario();
    let root = tmp.path();

    prove_plan_time_fan_out(root);

    // --- depends-on ordering, gate 1: the driver must pick
    // identity-contracts first — never identity-service while its upstream
    // is unmerged. -------------------------------------------------------
    assert_eq!(plan_next(root)["next"], "identity-contracts");
    // A second poll while contracts is in-progress returns the active
    // entry, never advancing to the dependent.
    let active = plan_next(root);
    assert_eq!(active["reason"], "in-progress");
    assert_eq!(active["active"], "identity-contracts");

    // --- Slice-time: drive identity-contracts (bound target: contracts). -
    drive_slice_to_built(root, "identity-contracts", "contracts", Sources::DocsOnly);

    // The contracts build request carries the adapter-declared `contracts`
    // input in `additional[]`; the bound target is `contracts`.
    let contracts_request =
        fs::read_to_string(root.join(".specify/slices/identity-contracts/build/request.yaml"))
            .expect("read contracts build request");
    assert!(
        contracts_request.contains("additional:") && contracts_request.contains("- contracts"),
        "contracts request resolves the declared `contracts` input into additional[], got:\n{contracts_request}"
    );

    specify_cmd()
        .current_dir(root)
        .args(["slice", "merge", "run", "identity-contracts"])
        .assert()
        .success();
    assert!(read_plan(root).contains("status: done"), "merge stamps the contracts entry done");
    // Upstream output is now visible in the shared baseline tree — the
    // in-tree dependency identity-service reads (no cross-slice channel).
    assert!(
        root.join(".specify/specs/identity/spec.md").is_file(),
        "contracts merge writes the shared baseline before the dependent starts"
    );

    // --- depends-on ordering, gate 2: only now does the driver advance to
    // identity-service. --------------------------------------------------
    assert_eq!(plan_next(root)["next"], "identity-service");

    // --- Slice-time: drive identity-service (bound target: omnia). ------
    drive_slice_to_built(root, "identity-service", "omnia", Sources::DocsAndLegacy);

    // The omnia build request declares no extra inputs, so additional[] is
    // absent (skip_serializing_if empty); the bound target is `omnia`.
    let service_request =
        fs::read_to_string(root.join(".specify/slices/identity-service/build/request.yaml"))
            .expect("read service build request");
    assert!(
        !service_request.contains("additional"),
        "omnia declares no extra inputs, so additional[] is omitted, got:\n{service_request}"
    );

    specify_cmd()
        .current_dir(root)
        .args(["slice", "merge", "run", "identity-service"])
        .assert()
        .success();

    // --- Final plan state: both driven slices done; the cross-source
    // password-reset slice remains pending (proven at plan time only). ---
    let final_plan = read_plan(root);
    let done = final_plan.matches("status: done").count();
    assert_eq!(
        done, 2,
        "both identity-contracts and identity-service reach done, got:\n{final_plan}"
    );
    assert!(
        final_plan.contains("status: pending"),
        "password-reset stays pending, got:\n{final_plan}"
    );
}

/// Which `(source, lead)` pairs a slice extracts Evidence for.
#[derive(Clone, Copy)]
enum Sources {
    DocsOnly,
    DocsAndLegacy,
}

/// Run `plan next --format json` and return the parsed body.
fn plan_next(root: &Path) -> Value {
    let out = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "plan", "next"])
        .assert()
        .success();
    parse_json(&out.get_output().stdout)
}

/// Create a slice bound to `target`, extract its Evidence, synthesize it,
/// assert the slice-time invariants, then build it to `built`.
fn drive_slice_to_built(root: &Path, slice: &str, target: &str, sources: Sources) {
    specify_cmd()
        .current_dir(root)
        .args(["slice", "create", slice, "--target", target])
        .assert()
        .success();

    // Fan-in #2: Evidence per (slice, source).
    extract_finalize(
        root,
        "docs",
        "documentation",
        "identity-api",
        slice,
        &fixture(&format!("evidence/{slice}/docs.yaml")),
    );
    if matches!(sources, Sources::DocsAndLegacy) {
        extract_finalize(
            root,
            "legacy",
            "code-typescript",
            "identity-api",
            slice,
            &fixture(&format!("evidence/{slice}/legacy.yaml")),
        );
    }
    let evidence_dir = root.join(format!(".specify/slices/{slice}/evidence"));
    assert!(evidence_dir.join("docs.yaml").is_file(), "{slice} docs Evidence persisted");
    if matches!(sources, Sources::DocsAndLegacy) {
        assert!(evidence_dir.join("legacy.yaml").is_file(), "{slice} legacy Evidence persisted");
    }

    // Synthesis: project the agent response into model.yaml + artifacts.
    fs::write(root.join("synth.json"), fixture(&format!("synthesis/{slice}.json")))
        .expect("write synth response");
    let synth = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "slice", "synthesize", slice, "--from", "synth.json"])
        .assert()
        .success();
    let artifacts: Vec<String> = parse_json(&synth.get_output().stdout)["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_string())
        .collect();
    for expected in ["proposal.md", "specs/identity/spec.md", "design.md", "tasks.md", "model.yaml"]
    {
        assert!(artifacts.contains(&expected.to_string()), "{slice} missing {expected}");
    }

    // model.yaml carries inline provenance; `slice validate` flags no
    // staleness; `slice provenance` projects the audit view.
    let validate = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "slice", "validate", slice])
        .assert();
    assert_no_staleness(validate.get_output());
    specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "slice", "provenance", slice])
        .assert()
        .success();

    // Contracts declares a required `contracts` build input; seed the
    // slice tree so request assembly resolves it.
    if target == "contracts" {
        let contracts_dir = root.join(format!(".specify/slices/{slice}/contracts"));
        fs::create_dir_all(&contracts_dir).expect("mkdir slice contracts");
        fs::write(contracts_dir.join("openapi.yaml"), fixture("contracts-input/openapi.yaml"))
            .expect("seed contracts input");
    }

    specify_cmd()
        .current_dir(root)
        .args(["slice", "transition", slice, "refined"])
        .assert()
        .success();

    // Build, prepare phase: assemble + schema-validate + persist request.
    let prepare = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "slice", "build", slice])
        .assert()
        .success();
    let handoff = parse_json(&prepare.get_output().stdout);
    assert_eq!(handoff["slice"], slice);
    assert_eq!(handoff["target"], target);
    assert_eq!(handoff["execution"], "agent");
    assert!(journal_has(root, "target.execution.agent"), "prepare emits target.execution.agent");

    // Build, finalize phase: validate the golden report + gate `built`.
    fs::write(
        root.join(format!(".specify/slices/{slice}/build/report.yaml")),
        fixture(&format!("reports/{slice}.yaml")),
    )
    .expect("write golden build report");
    let finalize = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "slice", "build", slice, "--phase", "finalize"])
        .assert()
        .success();
    let result = parse_json(&finalize.get_output().stdout);
    assert_eq!(result["status"], "success");
    assert!(journal_has(root, "slice.build.started"));
    assert!(journal_has(root, "slice.build.succeeded"));
    let meta = fs::read_to_string(root.join(format!(".specify/slices/{slice}/.metadata.yaml")))
        .expect("read slice metadata");
    assert!(meta.contains("status: built"), "finalize gates `built`, got:\n{meta}");
}

/// Assert the rendered `DiagnosticReport` on stdout carries no
/// slice-model / provenance staleness finding (RFC-29c §"Drift
/// validation"). Tolerates unrelated adapter findings — the D7
/// slice-time assertion is specifically "no staleness".
fn assert_no_staleness(output: &std::process::Output) {
    let Ok(report) = serde_json::from_slice::<Value>(&output.stdout) else {
        return;
    };
    let Some(findings) = report["findings"].as_array() else {
        return;
    };
    for rule in [
        "slice-model-schema",
        "slice-spec-provenance-stale",
        "slice-model-target-drift",
        "slice-model-source-orphan",
        "slice-model-cross-ref-orphan",
        "slice-model-claim-kind-mismatch",
        "slice-model-id-grammar",
    ] {
        assert!(
            findings.iter().all(|f| f["rule-id"] != rule),
            "staleness rule {rule} must not fire on a freshly synthesized slice: {findings:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Non-blocking determinism property (RFC-29d §"Non-blocking determinism")
// ---------------------------------------------------------------------------

/// Re-running kernel projection twice over a golden synthesis response
/// yields byte-identical kernel-owned `model.yaml` fields, and the
/// projection is target-independent: the same Evidence + response
/// projected for two slices bound to *different* targets yields
/// identical kernel-owned requirements, and the model carries no
/// target/adapter field.
#[test]
fn kernel_projection_deterministic() {
    let project = crate::common::Project::init().with_schemas();
    let root = project.root();

    // Two slices bound to different targets; `slice build` resolves the
    // target from `.metadata.yaml`, but the synthesis kernel never sees
    // it — that target-independence is what this test pins.
    project.seed_plan(
        "\
name: determinism
sources:
  docs:
    adapter: documentation
    path: ./docs
  legacy:
    adapter: code-typescript
    path: ./legacy
slices:
  - name: bound-contracts
    project: identity-contracts
    status: pending
    sources:
      - { source: docs, lead: identity-api }
  - name: bound-omnia
    project: identity-service
    status: pending
    sources:
      - { source: docs, lead: identity-api }
",
    );

    let evidence = fixture("evidence/identity-contracts/docs.yaml");
    let response = fixture("synthesis/identity-contracts.json");
    fs::write(root.join("synth.json"), &response).expect("write synth response");

    let mut requirements: Vec<Value> = Vec::new();
    for (slice, target) in [("bound-contracts", "contracts"), ("bound-omnia", "omnia")] {
        specify_cmd()
            .current_dir(root)
            .args(["slice", "create", slice, "--target", target])
            .assert()
            .success();
        let evidence_dir = project.slices_dir().join(format!("{slice}/evidence"));
        fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
        fs::write(evidence_dir.join("docs.yaml"), &evidence).expect("write evidence");

        specify_cmd()
            .current_dir(root)
            .args(["slice", "synthesize", slice, "--from", "synth.json"])
            .assert()
            .success();

        let show = specify_cmd()
            .current_dir(root)
            .args(["--format", "json", "slice", "model", "show", slice])
            .assert()
            .success();
        let model = parse_json(&show.get_output().stdout);
        assert!(model.get("target").is_none(), "kernel model carries no target field");
        assert!(model.get("adapter").is_none(), "kernel model carries no adapter field");
        requirements.push(model["requirements"].clone());
    }

    // Target-independence: the kernel-owned requirements are identical
    // across the two differently-targeted slices.
    assert_eq!(
        requirements[0], requirements[1],
        "kernel-owned requirements must be target-independent"
    );

    // Byte-identical re-projection: a second `--from` over the same golden
    // response reproduces the same `model.yaml` exactly.
    let model_path = project.slices_dir().join("bound-contracts/model.yaml");
    let first = fs::read_to_string(&model_path).expect("first model.yaml");
    specify_cmd()
        .current_dir(root)
        .args(["slice", "synthesize", "bound-contracts", "--from", "synth.json"])
        .assert()
        .success();
    let second = fs::read_to_string(&model_path).expect("second model.yaml");
    assert_eq!(first, second, "re-running projection must be byte-identical");
}
