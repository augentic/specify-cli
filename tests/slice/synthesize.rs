//! Slice synthesis engine — `slice synthesize` (RFC-29c M2b) plus the
//! acceptance / end-to-end coverage (RFC-29c C12).
//!
//! The kernel-level cases (normalize, orphan, divergence, determinism)
//! are unit-covered in `crates/workflow/src/slice/synthesis/*`; these
//! drive the same paths end-to-end through the built `slice synthesize`
//! command so the behaviour the `/spec:refine` skill consumes is the
//! behaviour under test. The drift-validator surface is owned by
//! `tests/slice_drift.rs`; here we only add the synthesized-slice happy
//! path it does not exercise.

use crate::support::*;

/// Evidence the synthesis kernel resolves authority and anchors claims
/// against. One `requirement` claim, behaviour authority.
const SYNTH_EVIDENCE_YAML: &str = "authority: behaviour
lead: my-slice
claims:
  - id: password-reset.request
    kind: requirement
    statement: \"The system lets a user request a reset link.\"
    path: src/users/reset.ts#L42
";

/// Agent synthesis response — one agreed requirement (single claim) and
/// one task. Kernel-owned fields omitted so the kernel projects them.
const SYNTH_RESPONSE_JSON: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Request password reset",
        "unit": "password-reset",
        "claims": [
          { "source": "legacy-monolith", "id": "password-reset.request", "kind": "requirement" }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// Stage a slice with one bound source's Evidence plus a plan entry, so
/// `slice synthesize` can read both the inline Evidence (dry-run) and
/// the on-disk Evidence the kernel resolves authority from (`--from`).
fn stage_synthesizable_slice() -> Project {
    let project = Project::init().with_schemas();
    specify_cmd()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    let slice_dir = project.slices_dir().join("my-slice");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("legacy-monolith.yaml"), SYNTH_EVIDENCE_YAML)
        .expect("write evidence");
    project.seed_plan(PLAN_WITH_LEGACY_MONOLITH);
    project
}

#[test]
fn synthesize_dry_run_emits_inputs_envelope() {
    let project = stage_synthesizable_slice();
    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--dry-run"])
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["kind"], "inputs");
    assert_eq!(value["slice"], "my-slice");
    let sources = value["sources"].as_array().expect("sources array");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["source"], "legacy-monolith");
    assert_eq!(sources[0]["lead"], "my-slice");
    assert!(
        !sources[0]["claims"].as_array().expect("claims array").is_empty(),
        "inline Evidence claims must be carried into the envelope"
    );
    assert!(
        !value["shape-brief"].as_str().expect("shape-brief string").is_empty(),
        "the resolved target shape brief must be embedded"
    );

    // Dry-run writes nothing.
    assert!(
        !project.slices_dir().join("my-slice/model.yaml").exists(),
        "dry-run must not write model.yaml"
    );

    // The always-agent / cache: opt-out signal fires on the dry-run.
    let journal =
        fs::read_to_string(project.root().join(".specify/journal.jsonl")).expect("read journal");
    assert!(
        journal.contains("slice.synthesize.agent"),
        "dry-run must emit slice.synthesize.agent, got:\n{journal}"
    );
}

#[test]
fn synthesize_from_projects_and_persists() {
    let project = stage_synthesizable_slice();
    let response_path = project.root().join("response.json");
    fs::write(&response_path, SYNTH_RESPONSE_JSON).expect("write response");

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--from"])
        .arg(&response_path)
        .assert()
        .success();

    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["slice"], "my-slice");
    let artifacts: Vec<String> = value["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_string())
        .collect();
    for expected in
        ["proposal.md", "specs/password-reset/spec.md", "design.md", "tasks.md", "model.yaml"]
    {
        assert!(artifacts.contains(&expected.to_string()), "missing {expected} in {artifacts:?}");
    }

    let slice_dir = project.slices_dir().join("my-slice");
    for rel in
        ["proposal.md", "design.md", "tasks.md", "model.yaml", "specs/password-reset/spec.md"]
    {
        assert!(slice_dir.join(rel).is_file(), "{rel} must be persisted");
    }

    // The persisted model.yaml is schema-valid: `slice model show`
    // loads it through `SliceModel::parse_yaml`, which schema-gates.
    let show = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let model = parse_json(&show.get_output().stdout);
    assert_eq!(model["slice"], "my-slice");
    assert_eq!(model["requirements"][0]["id"], "REQ-001");
    assert_eq!(model["requirements"][0]["status"], "agreed");
    assert_eq!(model["requirements"][0]["sources"][0], "legacy-monolith");

    // spec.md carries the kernel-rendered provenance lines.
    let spec = fs::read_to_string(slice_dir.join("specs/password-reset/spec.md")).expect("spec.md");
    assert!(spec.contains("ID: REQ-001"), "spec.md must carry the projected ID, got:\n{spec}");
    assert!(spec.contains("Sources: legacy-monolith"), "spec.md must carry Sources, got:\n{spec}");
    assert!(spec.contains("Status: agreed"), "spec.md must carry Status, got:\n{spec}");

    // The paired started/completed journal events bracket the write.
    let journal =
        fs::read_to_string(project.root().join(".specify/journal.jsonl")).expect("read journal");
    assert!(journal.contains("slice.synthesize.started"), "missing started, got:\n{journal}");
    assert!(journal.contains("slice.synthesize.completed"), "missing completed, got:\n{journal}");
}

#[test]
fn synthesize_requires_a_mode() {
    let project = stage_synthesizable_slice();
    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2));
    let value = parse_json(&assert.get_output().stderr);
    assert_eq!(value["error"], "slice-synthesize-mode-required");
}

/// Write `response_json` to `<root>/response.json` and run
/// `slice synthesize my-slice --from response.json`, returning the
/// process output for the caller to assert on.
fn run_synthesize_from(project: &Project, response_json: &str) -> std::process::Output {
    let response_path = project.root().join("response.json");
    fs::write(&response_path, response_json).expect("write response");
    specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--from"])
        .arg(&response_path)
        .assert()
        .get_output()
        .clone()
}

/// A response that pre-assigns every kernel-owned field to a wrong (but
/// schema-valid) value — `REQ-999`, `status: conflict`, a stray
/// `sources` list, a claim `winner`, and a bogus `model.slice` /
/// `model.project` header. The kernel must ignore each and re-derive the
/// canonical projection (RFC-29c §"Synthesis response": normalize, never
/// reject). The single in-Evidence claim is `agreed` once re-derived.
const SYNTH_RESPONSE_PRE_ASSIGNED: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "slice": "bogus-slice",
    "project": "bogus-project",
    "requirements": [
      {
        "id": "REQ-999",
        "title": "Request password reset",
        "status": "conflict",
        "unit": "password-reset",
        "sources": ["wrong-source"],
        "claims": [
          { "source": "legacy-monolith", "id": "password-reset.request", "kind": "requirement", "winner": true }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// A response whose claim cites an Evidence id (`ghost-claim`) absent
/// from `evidence/legacy-monolith.yaml` — the kernel cannot anchor it
/// and aborts `slice-model-source-orphan`.
const SYNTH_RESPONSE_ORPHAN: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Request password reset",
        "unit": "password-reset",
        "claims": [
          { "source": "legacy-monolith", "id": "ghost-claim", "kind": "requirement" }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// A response whose claim records `kind: criterion`, but the matching
/// Evidence claim `password-reset.request` is recorded as a
/// `requirement` — the kernel aborts `slice-model-claim-kind-mismatch`.
const SYNTH_RESPONSE_KIND_MISMATCH: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Request password reset",
        "unit": "password-reset",
        "claims": [
          { "source": "legacy-monolith", "id": "password-reset.request", "kind": "criterion" }
        ],
        "statement": "The system lets a registered user request a password reset link by email."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Implement password reset request handling.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Password reset\nWhy this slice exists.\n",
    "design": "# Design\nDomain model.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Request password reset\nAgent prose body.\n" }
    ]
  }
}
"###;

/// Plan binding two sources to `my-slice`: documentation-authority
/// `docs` and behaviour-authority `legacy`, both citing the same
/// `password-reset.expiry` claim. The RFC-29c §"Slice model (D4)"
/// worked divergence: the documentation `criterion` beats the behaviour
/// `example`.
const DIVERGENCE_PLAN: &str = "\
name: divergence
lifecycle: pending
sources:
  docs:
    adapter: documentation
    path: ./docs
  legacy:
    adapter: code-typescript
    path: ./legacy
slices:
  - name: my-slice
    status: pending
    project: test-proj
    sources:
      - { source: docs, lead: my-slice }
      - { source: legacy, lead: my-slice }
";

/// Documentation-authority Evidence: the criterion claim that wins the
/// divergence. The provenance projection reads its `value` / `path`.
const DIVERGENCE_EVIDENCE_DOCS: &str = "authority: documentation
lead: my-slice
claims:
  - id: password-reset.expiry
    kind: criterion
    criterion: Reset links expire after 30 minutes.
    path: docs/identity/reset.md#L7
";

/// Behaviour-authority Evidence: the example claim that loses the
/// divergence but survives in provenance with `winner: false`.
const DIVERGENCE_EVIDENCE_LEGACY: &str = "authority: behaviour
lead: my-slice
claims:
  - id: password-reset.expiry
    kind: example
    output: expiresAt = createdAt + 24h
    path: src/users/reset.ts#L88
";

/// Agent response for the divergence slice — one `disagreed`
/// requirement citing both sources' `password-reset.expiry` claim.
const DIVERGENCE_RESPONSE_JSON: &str = r###"{
  "version": 1,
  "kind": "response",
  "slice": "my-slice",
  "model": {
    "requirements": [
      {
        "title": "Reset link expiry",
        "unit": "password-reset",
        "agreement": "disagreed",
        "claims": [
          { "source": "docs", "id": "password-reset.expiry", "kind": "criterion" },
          { "source": "legacy", "id": "password-reset.expiry", "kind": "example" }
        ],
        "statement": "Reset links expire after 30 minutes."
      }
    ],
    "tasks": [
      { "id": "TASK-001", "text": "Enforce reset link expiry.", "satisfies": ["REQ-001"] }
    ]
  },
  "artifacts": {
    "proposal": "# Reset expiry\nWhy this slice exists.\n",
    "design": "# Design\nExpiry handling.\n",
    "tasks": "# Tasks\n- [ ] TASK-001\n",
    "specs": [
      { "unit": "password-reset", "content": "## Reset link expiry\nLinks expire after 30 minutes.\n" }
    ]
  }
}
"###;

/// Stage `my-slice` with two bound sources (docs + legacy) sharing the
/// `password-reset.expiry` claim, so the kernel resolves a per-kind
/// divergence.
fn stage_divergence_slice() -> Project {
    let project = Project::init().with_schemas();
    specify_cmd()
        .current_dir(project.root())
        .args(["slice", "create", "my-slice"])
        .assert()
        .success();
    let slice_dir = project.slices_dir().join("my-slice");
    let evidence_dir = slice_dir.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("mkdir evidence");
    fs::write(evidence_dir.join("docs.yaml"), DIVERGENCE_EVIDENCE_DOCS).expect("write docs");
    fs::write(evidence_dir.join("legacy.yaml"), DIVERGENCE_EVIDENCE_LEGACY).expect("write legacy");
    project.seed_plan(DIVERGENCE_PLAN);
    project
}

#[test]
fn synthesize_dry_run_omits_authority() {
    // The inputs envelope carries each source's inline `lead` + `claims`
    // and the resolved shape brief, but never the document-level
    // `authority` — the kernel resolves authority post-response (RFC-29c
    // §"Synthesis response").
    let project = stage_synthesizable_slice();
    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "synthesize", "my-slice", "--dry-run"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.clone();

    let value = parse_json(&stdout);
    assert_eq!(value["sources"][0]["lead"], "my-slice");
    assert!(
        !value["sources"][0]["claims"].as_array().expect("claims array").is_empty(),
        "inline Evidence claims must be carried"
    );
    assert!(!value["shape-brief"].as_str().expect("shape-brief").is_empty());

    // No `authority` key anywhere in the rendered envelope.
    let text = String::from_utf8(stdout).expect("utf8 stdout");
    assert!(
        !text.contains("authority"),
        "authority must be absent from the inputs envelope: {text}"
    );
}

#[test]
fn synthesize_from_no_provenance() {
    // RFC-29c §"Command": provenance is carried inline in `model.yaml`;
    // there is no persisted `provenance.yaml`.
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "synthesize --from must succeed");

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(slice_dir.join("model.yaml").is_file(), "model.yaml must be persisted");
    assert!(
        !slice_dir.join("provenance.yaml").exists(),
        "synthesize must never write a provenance.yaml"
    );
}

#[test]
fn synthesize_normalizes_fields() {
    // The agent pre-assigns wrong-but-valid kernel/header fields; the
    // command ignores them all and persists the canonical derivation
    // (RFC-29c §"Synthesis response": normalize, never reject).
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_PRE_ASSIGNED);
    assert_eq!(output.status.code(), Some(0), "a normalizing projection must succeed");

    let show = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let model = parse_json(&show.get_output().stdout);

    // Header re-stamped from the slice, not the agent's bogus values.
    assert_eq!(model["slice"], "my-slice");
    assert!(model.get("project").is_none() || model["project"].is_null());

    // Requirement fields re-derived: REQ-001 (not REQ-999), agreed (not
    // conflict), sources [legacy-monolith] (not wrong-source), and no
    // winner marker on the single agreed claim.
    let req = &model["requirements"][0];
    assert_eq!(req["id"], "REQ-001");
    assert_eq!(req["status"], "agreed");
    assert_eq!(req["sources"][0], "legacy-monolith");
    assert_eq!(req["sources"].as_array().expect("sources array").len(), 1);
    assert!(
        req["claims"][0].get("winner").is_none() || req["claims"][0]["winner"].is_null(),
        "an agreed single-claim requirement carries no winner marker"
    );
}

#[test]
fn synthesize_aborts_on_source_orphan() {
    // A claim that anchors no on-disk Evidence aborts the command before
    // any write, emitting the failure journal event (RFC-29c §"Persist
    // pipeline" step 1).
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_ORPHAN);
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output.stderr);
    assert_eq!(value["error"], "slice-model-source-orphan");

    let slice_dir = project.slices_dir().join("my-slice");
    assert!(!slice_dir.join("model.yaml").exists(), "an aborted synthesis writes nothing");

    let journal =
        fs::read_to_string(project.root().join(".specify/journal.jsonl")).expect("read journal");
    assert!(journal.contains("slice.synthesize.failed"), "abort must emit failed, got:\n{journal}");
    assert!(
        !journal.contains("slice.synthesize.completed"),
        "an aborted synthesis must not emit completed, got:\n{journal}"
    );
}

#[test]
fn synthesize_aborts_on_claim_kind_mismatch() {
    // A claim kind that disagrees with the kind Evidence records for the
    // same `(source, id)` aborts `slice-model-claim-kind-mismatch` (D13).
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_KIND_MISMATCH);
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output.stderr);
    assert_eq!(value["error"], "slice-model-claim-kind-mismatch");

    assert!(
        !project.slices_dir().join("my-slice/model.yaml").exists(),
        "an aborted synthesis writes nothing"
    );
}

#[test]
fn synthesize_resolves_per_kind_divergence() {
    // The RFC-29c worked divergence: a documentation `criterion` beats a
    // behaviour `example`. The command derives `status: divergence`, the
    // winner / loser markers, the rendered source order, and the
    // `[divergence]` spec tag.
    let project = stage_divergence_slice();
    let output = run_synthesize_from(&project, DIVERGENCE_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "the divergence slice synthesizes");

    let show = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "model", "show", "my-slice"])
        .assert()
        .success();
    let model = parse_json(&show.get_output().stdout);
    let req = &model["requirements"][0];
    assert_eq!(req["id"], "REQ-001");
    assert_eq!(req["status"], "divergence");
    // Documentation (docs) outranks behaviour (legacy), so docs renders
    // first and wins; legacy loses.
    assert_eq!(req["sources"][0], "docs");
    assert_eq!(req["sources"][1], "legacy");
    assert_eq!(req["claims"][0]["source"], "docs");
    assert_eq!(req["claims"][0]["winner"], true);
    assert_eq!(req["claims"][1]["source"], "legacy");
    assert_eq!(req["claims"][1]["winner"], false);

    // spec.md carries the `[divergence]` heading tag and the matching
    // Status line.
    let spec =
        fs::read_to_string(project.slices_dir().join("my-slice/specs/password-reset/spec.md"))
            .expect("spec.md");
    assert!(
        spec.contains("[divergence]"),
        "non-agreed status renders the heading tag, got:\n{spec}"
    );
    assert!(spec.contains("Status: divergence"), "spec.md must carry the projected status");
    assert!(spec.contains("Sources: docs, legacy"), "spec.md renders the ordered source list");
}

#[test]
fn synthesize_then_validate_is_drift_clean() {
    // A slice synthesized by the command must pass `slice validate`'s
    // typed-model drift gate: the command loaded and re-validated
    // `model.yaml`, so none of the seven RFC-29c §"Drift validation"
    // findings fire. (Crafted-bad-slice coverage lives in
    // `tests/slice_drift.rs`; this is the synthesized happy path.)
    let project = stage_synthesizable_slice();
    let output = run_synthesize_from(&project, SYNTH_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "synthesize must succeed before validate");

    let validate = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "validate", "my-slice"])
        .assert();
    let output = validate.get_output();
    for rule_id in [
        "slice-model-schema",
        "slice-spec-provenance-stale",
        "slice-model-target-drift",
        "slice-model-source-orphan",
        "slice-model-cross-ref-orphan",
        "slice-model-claim-kind-mismatch",
        "slice-model-id-grammar",
    ] {
        assert_no_finding(output, rule_id);
    }
}

#[test]
fn provenance_recomputes_labels() {
    // `slice provenance` over a synthesized divergence model recomputes
    // the `authority-resolved` label and reads each claim's `value` /
    // `path` from on-disk Evidence (RFC-29c §"Provenance projection").
    let project = stage_divergence_slice();
    let output = run_synthesize_from(&project, DIVERGENCE_RESPONSE_JSON);
    assert_eq!(output.status.code(), Some(0), "the divergence slice synthesizes");

    let prov = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "slice", "provenance", "my-slice"])
        .assert()
        .success();
    let index = parse_json(&prov.get_output().stdout);
    let req = &index["requirements"][0];
    assert_eq!(req["id"], "REQ-001");
    assert_eq!(req["status"], "divergence");
    // Recomputed, not read from the model.
    assert_eq!(req["resolution"], "authority-resolved");
    assert_eq!(req["resolution-trace"]["step"], "default-authority-ordering");
    assert_eq!(req["resolution-trace"]["winner"], "docs");

    // `value` / `path` are read from Evidence for both the winner and
    // the dropped loser.
    let claims = req["contributing-claims"].as_array().expect("contributing-claims array");
    let docs = claims.iter().find(|c| c["source"] == "docs").expect("docs claim");
    assert_eq!(docs["value"], "Reset links expire after 30 minutes.");
    assert_eq!(docs["path"], "docs/identity/reset.md#L7");
    assert_eq!(docs["winner"], true);
    let legacy = claims.iter().find(|c| c["source"] == "legacy").expect("legacy claim");
    assert_eq!(legacy["value"], "expiresAt = createdAt + 24h");
    assert_eq!(legacy["path"], "src/users/reset.ts#L88");
    assert_eq!(legacy["winner"], false);
}

#[test]
fn synthesize_from_is_deterministic() {
    // RFC-29c §"Kernel determinism": running `--from` twice over the
    // same response yields a byte-identical `model.yaml`. (The model
    // carries no timestamp, and the kernel is target-independent.)
    let project = stage_synthesizable_slice();
    let model_path = project.slices_dir().join("my-slice/model.yaml");

    assert_eq!(run_synthesize_from(&project, SYNTH_RESPONSE_JSON).status.code(), Some(0));
    let first = fs::read_to_string(&model_path).expect("first model.yaml");

    assert_eq!(run_synthesize_from(&project, SYNTH_RESPONSE_JSON).status.code(), Some(0));
    let second = fs::read_to_string(&model_path).expect("second model.yaml");

    assert_eq!(first, second, "model.yaml must be byte-identical across two synthesis runs");
}
