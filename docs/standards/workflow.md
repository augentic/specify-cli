# Workflow contract

The in-force contract this binary implements. Stable anchors that source code and skill briefs cite by `§`-name. The full historical motivation lives in the archived RFCs ([`rfc-25-workflow.md`](https://github.com/augentic/specify/blob/main/rfcs/archive/rfc-25-workflow.md), [`rfc-26-workflow.md`](https://github.com/augentic/specify/blob/main/rfcs/archive/rfc-26-workflow.md), [`rfc-27-synthesis.md`](https://github.com/augentic/specify/blob/main/rfcs/archive/rfc-27-synthesis.md)); this document is the live anchor surface and supersedes them as the contract.

## Adapter vocabulary

Two adapter roles — `source` (operations: `enumerate`, `extract`) and `target` (operations: `shape`, `build`, `merge`). The shared on-disk shape is `adapter.yaml`; per-axis schemas refine it. See the parent repo's [`AGENTS.md` §"Vocabulary"](https://github.com/augentic/specify/blob/main/AGENTS.md#vocabulary).

## Adapter implementation shape

Per-adapter `adapter.yaml` carries `name`, `version`, `axis`, `description`, the `briefs.<operation>` map, and an optional `tools[]` array. The closed operation set is determined by the manifest's `axis`. Implementation: [`crates/domain/src/adapter/`](../../crates/domain/src/adapter); per-axis schemas at [`schemas/adapter.schema.json`](../../schemas/adapter.schema.json), [`source.schema.json`](../../schemas/source.schema.json), [`target.schema.json`](../../schemas/target.schema.json).

## Source adapter contract

`axis: source`; `briefs.keys() ⊆ {enumerate, extract}`. `enumerate` writes `## Candidate inventory` blocks under `discovery.md` at plan time; `extract` writes one Evidence document per `(source-key, candidate-id)` pair at slice time. See [`schemas/source.schema.json`](../../schemas/source.schema.json) and [`schemas/evidence.schema.json`](../../schemas/evidence.schema.json).

## Target adapter contract

`axis: target`; `briefs.keys() ⊆ {shape, build, merge}`. `shape` is read by core synthesis; `build` and `merge` are agent-driven. See [`schemas/target.schema.json`](../../schemas/target.schema.json).

## Resolver and cache

`SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points. Probe order:

1. `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/` — agent-populated mirror.
2. `<project_dir>/adapters/{sources,targets}/<name>/` — in-repo manifest.

The `{sources,targets}` segment is keyed by `Axis`. See [`DECISIONS.md` §"Adapter loader axis routing"](../../DECISIONS.md#adapter-loader-axis-routing) and [`DECISIONS.md` §"Cache layout"](../../DECISIONS.md#cache-layout).

## Adapter name uniqueness

A name appears under `adapters/sources/<name>/` xor `adapters/targets/<name>/`. Collisions surface as `adapter-name-axis-collision`. See [`DECISIONS.md` §"Adapter name uniqueness"](../../DECISIONS.md#adapter-name-uniqueness).

## Discovery handshake

`enumerate` writes `## Candidate inventory` blocks; the operator stamps `reviewed`; `extract` resolves `slices[].sources[].candidate` against `id`-then-`aliases[]`. Schema at [`schemas/discovery/candidate.schema.json`](../../schemas/discovery/candidate.schema.json); parser at [`crates/domain/src/discovery/document.rs`](../../crates/domain/src/discovery/document.rs).

## The Plan

`plan.yaml` shape is fixed by [`schemas/plan/plan.schema.json`](../../schemas/plan/plan.schema.json). Two stored lifecycle states (`pending | reviewed`); per-entry status is `pending | in-progress | done`. Writer ownership is split — see §"Writer ownership" below.

## Workflow vocabulary

`Slice`, `Candidate`, `Evidence`, `Source`, `Target`, `Plan`, `Discovery`. Definitions live in the parent repo's [`AGENTS.md` §"Vocabulary"](https://github.com/augentic/specify/blob/main/AGENTS.md#vocabulary).

## Plan-time fusion

Core synthesis fuses candidates across sources at plan time and writes one `slices[]` row per fusion outcome. The closed `Divergence` enum (`none | likely | accepted | rejected`) records the fusion outcome's confidence. See [`DECISIONS.md` §"`Divergence` enum"](../../DECISIONS.md#divergence-enum) and [`crates/domain/src/change/plan/core/model.rs`](../../crates/domain/src/change/plan/core/model.rs).

## Source

`plan.yaml.sources.<key>` is the structured `{ adapter, path?, value? }` object with exactly one of `path` / `value`. See [`DECISIONS.md` §"Plan source bindings"](../../DECISIONS.md#plan-source-bindings).

`Slice.sources` (a slice's per-source binding list) accepts the bare-string shorthand on parse and serialises as the structured `{ key, candidate }`. See [`DECISIONS.md` §"`SliceSourceBinding`: bare shorthand plus structured form"](../../DECISIONS.md#slicesourcebinding-bare-shorthand-plus-structured-form).

## Authority hierarchy

Closed enum `intent > documentation > behaviour`. Resolution order: per-slice override (D3) → per-Evidence per-kind override (D2) → Evidence document-level `authority:` → conflict. Implementation: [`crates/domain/src/evidence/authority.rs`](../../crates/domain/src/evidence/authority.rs).

## Execution model

`pending → reviewed` plan-level (Gate 1; operator-only). Per-entry: `pending → in-progress → done`. `done` is absorbing in v1; the operator-reversed flow lives behind `specrun plan transition --undo` (added Phase 6 — see [`DECISIONS.md` §"Plan lifecycle: two stored states"](../../DECISIONS.md#plan-lifecycle-two-stored-states)).

## Refinement

`/spec:refine` runs `extract` per bound source, synthesizes `proposal.md` / `spec.md` / `design.md` / `tasks.md`, writes `fusion.yaml`, and transitions the slice to `refined`. Validators live in [`crates/domain/src/validate/`](../../crates/domain/src/validate/) and [`src/commands/slice/validate.rs`](../../src/commands/slice/validate.rs).

## Extraction

Per-source `extract` is keyed on a closed five-input fingerprint; results cached at `.specify/.cache/extractions/<adapter>/<fingerprint>/`. See §D8 below.

## Requirement block contract

`spec.md` requirements carry `ID:` / `Sources:` / `Status:` metadata; the closed `RequirementStatus` enum is `agreed | unknown | conflict | divergence`. Parser at [`crates/domain/src/spec/provenance.rs`](../../crates/domain/src/spec/provenance.rs).

## Wire format

Kebab-case discriminants on the JSON envelope; `snake_case` Rust variants bridge to the wire via `#[serde(rename = "…")]`. Lifecycle values, claim kinds, divergence enum, authority enum — all kebab on the wire. See [`DECISIONS.md` §"Wire compatibility"](../../DECISIONS.md#wire-compatibility).

## Sandboxing

WASI tool runner pre-opens `$PROJECT_DIR` always, `$CAPABILITY_DIR` only for plugin-scope tools. No host environment leaks. See [`DECISIONS.md` §"`$CAPABILITY_DIR` replaces `$ADAPTER_DIR`"](../../DECISIONS.md#capability_dir-replaces-adapter_dir).

## CLI surface

Headline verbs: `init`, `source resolve`, `target resolve`, `slice {create, transition, validate, merge}`, `plan {create, add, amend, transition, next, finalize}`, `workspace {sync, push, prepare}`, `tool run`. See [`specify --help`](../init.md) and the parent repo's [`AGENTS.md` §"Skill / CLI responsibility split"](https://github.com/augentic/specify/blob/main/AGENTS.md#skill--cli-responsibility-split).

## Writer ownership

Per-entry status writes route to exactly one CLI verb each — `plan add` / `plan amend` write `pending`, `plan next` writes `in-progress`, `slice merge` (via `plan transition <entry> done`) writes `done`. Plan-level `reviewed` is operator-only. See [`DECISIONS.md` §"Lifecycle write-ownership"](../../DECISIONS.md#lifecycle-write-ownership).

## Observability

Newline-delimited JSON journal at `.specify/journal.jsonl`. The closed `EventKind` taxonomy lives in [`crates/domain/src/journal.rs`](../../crates/domain/src/journal.rs); the per-event table is in [`DECISIONS.md` §"Journal event names"](../../DECISIONS.md#journal-event-names).

## Operations typed at parse boundary

`briefs.keys()` is the canonical operation iterator; the closed `SourceOperation` / `TargetOperation` enums in [`crates/domain/src/adapter/operation.rs`](../../crates/domain/src/adapter/operation.rs) are the typed key set. See [`DECISIONS.md` §"Operations typed at parse boundary"](../../DECISIONS.md#operations-typed-at-parse-boundary).

## What was cut and why

`specify adapter *` and `specify change *` retired at 2.0; no compatibility aliases. The 1.x bare-string `sources` shorthand on `plan.yaml.sources.<key>` is gone. See [`DECISIONS.md` §"Plan source bindings"](../../DECISIONS.md#plan-source-bindings).

## Note to the implementing agent

Touching `Slice.target`, `SliceSourceBinding`, `Divergence`, `crates/domain/src/spec/provenance.rs`, `crates/domain/src/adapter/`, `crates/domain/src/journal.rs`, `crates/domain/src/schema.rs`, the `$CAPABILITY_DIR` env var, or the `plugin--<axis>--<slug>` tool cache scope requires a cross-repo `rg` sweep against both [`augentic/specify-cli`](https://github.com/augentic/specify-cli) and [`augentic/specify`](https://github.com/augentic/specify) in the same PR — the contract spans both repos.

## D1 — Runtime source adapter (`captures`)

`captures` emits `kind: example` Evidence claims with `replay-digest: sha256:…` anchors and default `authority: behaviour`. Schema entry in [`schemas/evidence.schema.json`](../../schemas/evidence.schema.json); claim type at [`crates/domain/src/evidence/claim/example.rs`](../../crates/domain/src/evidence/claim/example.rs).

## D2 — Per-kind authority on Evidence

`evidence.schema.json` carries an optional `authority-overrides` map keyed by claim kind. Synthesis consults this map before the document-level `authority:`. See [`DECISIONS.md` §"RFC-27 §D2 — per-kind authority on Evidence"](../../DECISIONS.md#rfc-27-d2--per-kind-authority-on-evidence) and [`crates/domain/src/evidence/authority.rs`](../../crates/domain/src/evidence/authority.rs).

## D3 — Per-slice authority on `plan.yaml`

`plan.yaml.slices[].authority-override` maps claim kind to a source key bound on the slice. Orphan keys surface as `slice-authority-override-orphan-source-key`. See [`DECISIONS.md` §"RFC-27 §D3 — per-slice authority on `plan.yaml`"](../../DECISIONS.md#rfc-27-d3--per-slice-authority-on-planyaml).

## D4 — `fusion.yaml` is audit-only

Reconciliation index at `.specify/slices/<slice>/fusion.yaml`; `spec.md` is the authoritative artifact. Schema at [`schemas/slice/fusion.schema.json`](../../schemas/slice/fusion.schema.json); validator at [`crates/domain/src/slice/fusion.rs`](../../crates/domain/src/slice/fusion.rs). See [`DECISIONS.md` §"RFC-27 §D4 — `fusion.yaml` is audit-only"](../../DECISIONS.md#rfc-27-d4--fusionyaml-is-audit-only).

## D5 — Operator-driven `divergence`

The CLI is the single writer of every `Divergence` variant. Operators flip `accepted | rejected` via `specrun plan amend --divergence`; `likely` is staged by `specrun plan create --divergence-likely <slice>`. See [`crates/domain/src/change/plan/core/model.rs`](../../crates/domain/src/change/plan/core/model.rs).

## D6 — Discovery aliases

Candidates carry an optional `aliases[]` bullet. `slices[].sources[].candidate` resolves first against `id`, then against any entry in `aliases[]`. Aliases live in a single namespace per `discovery.md`. Parser at [`crates/domain/src/discovery/document.rs`](../../crates/domain/src/discovery/document.rs); collision discriminant `discovery-alias-collision`.

## D7 — `--auto-review`

`specrun plan create --auto-review` stamps Gate 1 in the same invocation when validation passes. Failure under `--auto-review` MUST NOT stamp; the operator re-runs after fixing.

## D8 — Cache fingerprint inputs

Closed five-input list: source path canonicalised, adapter `name@version`, brief sha256, sorted declared-tool versions, candidate id. Cache at `.specify/.cache/extractions/<adapter>/<fingerprint>/` with append-only `index.jsonl` at the adapter root. Implementation at [`crates/domain/src/adapter/cache.rs`](../../crates/domain/src/adapter/cache.rs); see [`DECISIONS.md` §"RFC-27 §D8 — cache fingerprint inputs"](../../DECISIONS.md#rfc-27-d8--cache-fingerprint-inputs).

## Reconciliation index

Closed top-level shape on `fusion.yaml`: `version`, `slice`, `generated-at`, `generator`, `requirements[]`. See [`crates/domain/src/slice/fusion.rs`](../../crates/domain/src/slice/fusion.rs) and §D4 above.
