# Workflow contract

The in-force contract this binary implements. Stable anchors that source code and skill briefs cite by `§`-name. This document is the live anchor surface for workflow behavior.

## Adapter vocabulary

Two adapter roles — `source` (operations: `survey`, `extract`) and `target` (operations: `shape`, `build`, `merge`). The shared on-disk shape is `adapter.yaml`; per-axis schemas refine it. See the parent repo's [`AGENTS.md` §"Vocabulary"](https://github.com/augentic/specify/blob/main/AGENTS.md#vocabulary).

## Adapter implementation shape

Per-adapter `adapter.yaml` carries `name`, `version`, `axis`, the required closed `execution` mode (`agent` | `tool`, RFC-29 D9), `description`, the `briefs.<operation>` map, an optional `cache` opt-out, and an optional `tools[]` array. The closed operation set is determined by the manifest's `axis`. `execution: agent` forces `cache: opt-out`; the loader rejects a manifest that omits `execution` (`adapter-execution-mode-required`) or declares `execution: agent` alongside a non-opt-out cache mode (`adapter-execution-agent-cache-conflict`). Implementation: [`crates/workflow/src/adapter/`](../../crates/workflow/src/adapter); per-axis schemas at [`schemas/adapter.schema.json`](../../schemas/adapter.schema.json), [`source.schema.json`](../../schemas/source.schema.json), [`target.schema.json`](../../schemas/target.schema.json).

## Source adapter contract

`axis: source`; `briefs.keys() ⊆ {extract, survey}`. `survey` writes `## Lead inventory` blocks under `discovery.md` at plan time; `extract` writes one Evidence document per `(source, lead)` pair at slice time. See [`schemas/source.schema.json`](../../schemas/source.schema.json) and [`schemas/evidence.schema.json`](../../schemas/evidence.schema.json).

`specrun source survey <source> [--plan <name>] [--phase prepare|finalize]` and `specrun source extract <source> <lead> --slice <slice> [--phase prepare|finalize]` are the CLI-owned runners (RFC-29 D1). `<source>` resolves against `plan.yaml.sources.<key>`, then the adapter from `SourceBinding.adapter`. Both validate before the write becomes visible (lead set against `schemas/discovery/lead.schema.json` then `discovery.md` merge; Evidence against `schemas/evidence.schema.json` then persist to `.specify/slices/<slice>/evidence/<source>.yaml`). Under `execution: agent` dispatch is two-phase (`prepare` builds the sandbox + prints the handoff envelope; `finalize` validates / persists / caches / journals); under `execution: tool` a single call runs the whole operation. Value-bound sources (`intent`) carry `value-inline`; path bindings carry `source-path`. See [`DECISIONS.md` §"Source operations (D1)"](../../DECISIONS.md#source-operations-d1) and [`DECISIONS.md` §"Adapter execution mode (D9)"](../../DECISIONS.md#adapter-execution-mode-d9).

## Target adapter contract

`axis: target`; `briefs.keys() ⊆ {shape, build, merge}`. `shape` is read by core synthesis; `build` and `merge` are agent-driven. See [`schemas/target.schema.json`](../../schemas/target.schema.json).

## Resolver and cache

`SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points. Probe order:

1. `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/` — agent-populated mirror.
2. `<project_dir>/adapters/{sources,targets}/<name>/` — in-repo manifest.

The `{sources,targets}` segment is keyed by `Axis`. See [`DECISIONS.md` §"Adapter loader axis routing"](../../DECISIONS.md#adapter-loader-axis-routing) and [`DECISIONS.md` §"Cache layout"](../../DECISIONS.md#cache-layout).

The `source resolve` / `target resolve` JSON envelope carries `briefs-dir` — the absolute path to the resolved adapter's `briefs/` directory — alongside `resolved-path`, `operations`, and `description`. The source-operation prep seam consumes it for brief-directory resolution.

## Adapter name uniqueness

A name appears under `adapters/sources/<name>/` xor `adapters/targets/<name>/`. Collisions surface as `adapter-name-axis-collision`. See [`DECISIONS.md` §"Adapter name uniqueness"](../../DECISIONS.md#adapter-name-uniqueness).

## Discovery handshake

`survey` writes `## Lead inventory` blocks — one **raw, unmerged** lead per source, each identified by its `(source, lead)` pair (`survey` stamps `source` from the surveyed source). A re-survey of one source replaces only that source's blocks by `(source, lead)`; the same `lead` may appear under different source keys, and the lead/alias namespace is scoped per `source`. The operator stamps `approved`; `extract` resolves `slices[].sources[].lead` against `lead`-then-`aliases[]` within the binding's `source`. Cross-source unification is deferred to plan-time reconciliation (D2). Schema at [`schemas/discovery/lead.schema.json`](../../schemas/discovery/lead.schema.json); parser at [`crates/model/src/discovery/document.rs`](../../crates/model/src/discovery/document.rs).

## The Plan

`plan.yaml` shape is fixed by [`schemas/plan/plan.schema.json`](../../schemas/plan/plan.schema.json). Two stored lifecycle states (`pending | approved`); per-entry status is `pending | in-progress | done`. Writer ownership is split — see §"Writer ownership" below.

## Workflow vocabulary

`Slice`, `Lead`, `Evidence`, `Source`, `Target`, `Plan`, `Discovery`. Definitions live in the parent repo's [`AGENTS.md` §"Vocabulary"](https://github.com/augentic/specify/blob/main/AGENTS.md#vocabulary).

## Plan-time reconciliation

`specrun plan propose` reconciles surveyed leads across sources at plan time and writes the `plan.yaml.slices[]` rows (RFC-29 D2). `--dry-run [--format json]` reads `plan.yaml.sources`, the `discovery.md` lead inventory, and the project topology, then emits the `kind: request` envelope (flat `(source, lead)` lead catalog + `projects[]`) for the agent; it writes nothing. `--from <response.json> [--format json]` is the only slice writer: it schema-gates the response (`PROPOSAL_JSON_SCHEMA` at [`schemas/discovery/proposal.schema.json`](../../schemas/discovery/proposal.schema.json), kebab wire fields, closed `kind: request | response`), re-reads `discovery.md`, validates the partition over scopes (total coverage, at most one lead per source, fan-out `sources[]` consistency), derives slice names, binds each slice's `project` (the target adapter is resolved on demand from that project, never written to `plan.yaml`), and replaces `slices[]` only on a replaceable plan (`lifecycle: pending` and every entry `pending`). Cross-source matching is agent judgment; the operator curates at Gate 1. The closed `plan-reconcile-*` / `plan-propose-mode-required` codes are `Error::Validation` outcomes (exit 2). See [`DECISIONS.md` §"Lead reconciliation (D2)"](../../DECISIONS.md#lead-reconciliation-d2) and [`crates/workflow/src/change/plan/core/propose.rs`](../../crates/workflow/src/change/plan/core/propose.rs).

The closed `Divergence` enum (`none | likely | accepted | rejected`) records a reconciliation outcome's confidence. See [`DECISIONS.md` §"`Divergence` enum"](../../DECISIONS.md#divergence-enum) and [`crates/workflow/src/change/plan/core/model.rs`](../../crates/workflow/src/change/plan/core/model.rs).

## Source

`plan.yaml.sources.<key>` is the structured `{ adapter, path?, value? }` object with exactly one of `path` / `value`. See [`DECISIONS.md` §"Plan source bindings"](../../DECISIONS.md#plan-source-bindings).

`Slice.sources` (a slice's per-source binding list) accepts the bare-string shorthand on parse and serialises as the structured `{ source, lead }`. See [`DECISIONS.md` §"`SliceSourceBinding`: bare shorthand plus structured form"](../../DECISIONS.md#slicesourcebinding-bare-shorthand-plus-structured-form).

## Authority hierarchy

Closed enum `intent > documentation > behaviour`. v1 resolution order per `(source, kind)`: per-slice `authority-override` → Evidence document-level `authority:` → tie at the top class is a `conflict` (the per-Evidence per-kind override is deferred — see §"D2 — Per-kind authority on Evidence (deferred)"). The kernel resolves authority **after** the synthesis response returns and projects winners/`status` from it (§"Slice synthesis (RFC-29 M2b)"). Closed enums at [`crates/model/src/evidence/authority.rs`](../../crates/model/src/evidence/authority.rs); the production resolver at [`crates/workflow/src/slice/synthesis/authority.rs`](../../crates/workflow/src/slice/synthesis/authority.rs).

## Execution model

`pending → approved` plan-level (Gate 1; operator-only). Per-entry: `pending → in-progress → done`. `done` is absorbing in v1; the operator-reversed flow lives behind `specrun plan transition --undo` (added Phase 6 — see [`DECISIONS.md` §"Plan lifecycle: two stored states"](../../DECISIONS.md#plan-lifecycle-two-stored-states)).

## Refinement

`/spec:refine` runs `extract` per bound source, drives `specrun slice synthesize` (§"Slice synthesis (RFC-29 M2b)") to produce `proposal.md` / `spec.md` / `design.md` / `tasks.md` / `model.yaml` (provenance is carried inline in the single `model.yaml` artifact, projected on demand by `specrun slice provenance`), and transitions the slice to `refined`. Validators live in [`crates/validate/src/`](../../crates/validate/src/) and [`src/runtime/commands/slice/validate.rs`](../../src/runtime/commands/slice/validate.rs).

## Slice synthesis (RFC-29 M2b)

`specrun slice synthesize <slice>` turns a slice's `Evidence[]` into its requirement set, the single `model.yaml`, and the rendered Markdown artifacts (RFC-29 D3/D8/D10/D13). It mirrors `plan propose`'s two mutually-exclusive modes, exactly one of which is required (neither fails `slice-synthesize-mode-required`; the parser rejects both):

- `--dry-run [--format json]` is read-only: it reads each bound source's inline `lead` + `claims` from `evidence/<source>.yaml` and the resolved target `shape` brief body, then emits the agent **inputs** envelope (`kind: inputs`). Authority is **not** included. It writes nothing and emits `slice.synthesize.agent` (synthesis is always agent-dispatched, `cache: opt-out` — no tool path, no closed *request* wire shape).
- `--from <response.json> [--format json]` is the only writer: it schema-gates the response against `synthesis.schema.json` (`kind: response`, code `synthesis-schema`), resolves authority from on-disk Evidence + per-slice `authority-override`, runs the projection kernel, renders provenance lines into `specs/<unit>/spec.md`, drift-validates, then atomically/staged-persists `proposal.md` / `specs/<unit>/spec.md` / `design.md` / `tasks.md` / `model.yaml` (prior artifacts intact on failure). It emits `slice.synthesize.started` then `slice.synthesize.completed` (or `slice.synthesize.failed`). No `provenance.yaml` is ever written.

**Kernel ownership (normalize, never reject).** The agent authors per-requirement `claims[]` `(source, id, kind)`, an `agreement` verdict, prose (`title` / `statement` / `scenarios` / `notes`), the owning `unit`, the agent-authored `tasks[]` with `TASK` ids, and prose-only spec bodies (no `ID:` / `Sources:` / `Status:` lines). The kernel owns and re-derives the `version` / `slice` / `project` header, `REQ-NNN` ids (declaration order, no holes), `status`, per-claim `winner` markers, the rendered `sources` lists (highest authority first), and the inline provenance; any agent-supplied `id` / `status` / `winner` / `sources` is ignored and recomputed. Modules at [`crates/workflow/src/slice/synthesis/`](../../crates/workflow/src/slice/synthesis). Schema gate at [`crates/workflow/src/schema.rs`](../../crates/workflow/src/schema.rs) (`validate_synthesis_json`); `model.schema.json` and `synthesis.schema.json` are registered together through a `jsonschema::Registry` so the relative `model` `$ref` resolves. `specrun slice model show <slice> [--format json]` is the read-only model viewer. See [`DECISIONS.md` §"Slice synthesis engine (RFC-29 M2b)"](../../DECISIONS.md#slice-synthesis-engine-rfc-29-m2b).

**Drift validators.** `specrun slice validate` adds seven blocking typed-model findings (exit 2), emitted as `Diagnostic` findings on the `DiagnosticReport` surface:

| Finding | Meaning |
|---|---|
| `slice-model-schema` | `model.yaml` does not match `schemas/slice/model.schema.json`. |
| `slice-spec-provenance-stale` | Kernel-rendered provenance lines in `spec.md` disagree with `model.yaml`. |
| `slice-model-target-drift` | `model.yaml.project` disagrees with `plan.yaml.slices[<slice>].project`. (`target` is not persisted, so there is no target half.) |
| `slice-model-source-orphan` | A claim references an absent source key or Evidence claim id. |
| `slice-model-cross-ref-orphan` | A `satisfies[]` `REQ-*` reference is missing from `requirements[].id`. |
| `slice-model-claim-kind-mismatch` | A claim `kind` (D13) disagrees with the Evidence kind for that `(source, id)`. |
| `slice-model-id-grammar` | A `REQ` or `TASK` id does not match its closed three-digit grammar. |

## Extraction

Per-source `extract` is keyed on a closed five-input fingerprint; results cached at `.specify/.cache/extractions/<adapter>/<fingerprint>/`. See lint exit mapping below.

## Requirement block contract

`spec.md` requirements carry `ID:` / `Sources:` / `Status:` metadata; the closed `RequirementStatus` enum is `agreed | unknown | conflict | divergence`. Parser at [`crates/model/src/spec/provenance.rs`](../../crates/model/src/spec/provenance.rs).

## Wire format

Kebab-case discriminants on the JSON envelope; `snake_case` Rust variants bridge to the wire via `#[serde(rename = "…")]`. Lifecycle values, claim kinds, divergence enum, authority enum — all kebab on the wire. See [`DECISIONS.md` §"Wire compatibility"](../../DECISIONS.md#wire-compatibility).

## Sandboxing

WASI tool runner pre-opens `$PROJECT_DIR` always, `$CAPABILITY_DIR` only for plugin-scope tools. No host environment leaks. See [`DECISIONS.md` §"`$CAPABILITY_DIR` replaces `$ADAPTER_DIR`"](../../DECISIONS.md#capability_dir-replaces-adapter_dir).

Source-operation runners (`survey` / `extract`) preopen a four-root sandbox: `$SOURCE_DIR` read-only (absent for value-bound sources), `$CAPABILITY_DIR` read-only (manifest cache), `$SCRATCH_DIR` write-only, and `$PROJECT_DIR` **not visible**. Scratch nests disjoint from the result cache — `extract` under `.specify/.cache/extractions/<adapter>/<slice>/scratch/`, `survey` under `.specify/.cache/extractions/<adapter>/survey/scratch/`. See [`DECISIONS.md` §"Source operations (D1)"](../../DECISIONS.md#source-operations-d1).

## CLI surface

Headline verbs: `init`, `source {resolve, survey, extract, preview}`, `target resolve`, `slice {create, synthesize, model show, transition, validate, provenance, merge}`, `plan {create, propose, add, amend, transition, next, finalize}`, `workspace {sync, push, prepare}`, `tool run`, `journal emit`. See [`specify --help`](../init.md) and the parent repo's [`AGENTS.md` §"Skill / CLI responsibility split"](https://github.com/augentic/specify/blob/main/AGENTS.md#skill--cli-responsibility-split).

## Writer ownership

Per-entry status writes route to exactly one CLI verb each — `plan add` / `plan amend` write `pending`, `plan next` writes `in-progress`, `slice merge` (via `plan transition <entry> done`) writes `done`. Plan-level `approved` is operator-only. See [`DECISIONS.md` §"Lifecycle write-ownership"](../../DECISIONS.md#lifecycle-write-ownership).

## Observability

Newline-delimited JSON journal at `.specify/journal.jsonl`. The closed `EventKind` taxonomy lives in [`crates/workflow/src/journal.rs`](../../crates/workflow/src/journal.rs); the per-event table is in [`DECISIONS.md` §"Journal event names"](../../DECISIONS.md#journal-event-names). Source operations add `source.survey.cache-hit` / `.cache-miss` and `source.execution.agent`. `specrun slice synthesize` adds `slice.synthesize.{started,agent,completed,failed}` (§"Slice synthesis (RFC-29 M2b)"), distinct from the per-requirement `slice.synthesis.{conflict,divergence,unknown}` tag events. `specrun plan propose --from` emits a single `plan.reconcile.completed` event on a successful write (RFC-29 review F8 folded the former `plan.reconcile.agent` + `plan.reconcile.completed` pair into one indivisible event). Agent-orchestrated phases that lack a deterministic emit command write through `specrun journal emit <event-id> [--payload <json>]` — a guarded front door onto the same closed taxonomy, errors `journal-emit-unknown-event` / `journal-emit-payload-schema` (exit 2). See [`DECISIONS.md` §"`specrun journal emit` — guarded front door (D12)"](../../DECISIONS.md#specrun-journal-emit--guarded-front-door-d12).

## Operations typed at parse boundary

`briefs.keys()` is the canonical operation iterator; the closed `SourceOperation` / `TargetOperation` enums in [`crates/workflow/src/adapter/operation.rs`](../../crates/workflow/src/adapter/operation.rs) are the typed key set. See [`DECISIONS.md` §"Operations typed at parse boundary"](../../DECISIONS.md#operations-typed-at-parse-boundary).

## What was cut and why

`specify adapter *` and `specify change *` retired at 2.0; no compatibility aliases. The 1.x bare-string `sources` shorthand on `plan.yaml.sources.<key>` is gone. See [`DECISIONS.md` §"Plan source bindings"](../../DECISIONS.md#plan-source-bindings).

## Note to the implementing agent

Touching `Slice.target`, `SliceSourceBinding`, `Divergence`, `crates/model/src/spec/provenance.rs`, `crates/workflow/src/adapter/`, `crates/workflow/src/journal.rs`, `crates/workflow/src/schema.rs`, the `$CAPABILITY_DIR` env var, or the `plugin--<axis>--<slug>` tool cache scope requires a cross-repo `rg` sweep against both [`augentic/specify-cli`](https://github.com/augentic/specify-cli) and [`augentic/specify`](https://github.com/augentic/specify) in the same PR — the contract spans both repos.

## D1 — Runtime source adapter (`captures`)

`captures` emits `kind: example` Evidence claims with `replay-digest: sha256:…` anchors and default `authority: behaviour`. Schema entry in [`schemas/evidence.schema.json`](../../schemas/evidence.schema.json); claim type at [`crates/model/src/evidence/claim/example.rs`](../../crates/model/src/evidence/claim/example.rs).

## D2 — Per-kind authority on Evidence (deferred)

A per-Evidence `authority-overrides` map keyed by claim kind is **deferred to a future RFC** (decision-log §"Authority: document-level plus one override (v1)"). v1 resolves authority at document level via the Evidence `authority:` field, with the per-slice `authority-override` on `plan.yaml` as the sole override surface (D3). See [`DECISIONS.md` §"Authority: document-level plus one override (v1)"](../../DECISIONS.md#authority-document-level-plus-one-override-v1) and [`crates/model/src/evidence/authority.rs`](../../crates/model/src/evidence/authority.rs).

## D3 — Per-slice authority on `plan.yaml`

`plan.yaml.slices[].authority-override` maps claim kind to a source key bound on the slice. Orphan keys surface as `slice-authority-override-orphan-source`. See [`DECISIONS.md` §"Plan per-slice authority overrides"](../../DECISIONS.md#plan-per-slice-authority-overrides).

## D4 — Provenance is an on-demand projection

Provenance is carried inline in the single `model.yaml` artifact; `spec.md` is the authoritative artifact. There is no persisted `provenance.yaml` — `specrun slice provenance <slice> [--format]` projects the audit view on demand. Projection schema at [`schemas/slice/provenance.schema.json`](../../schemas/slice/provenance.schema.json); projector at [`crates/workflow/src/slice/provenance.rs`](../../crates/workflow/src/slice/provenance.rs). See [`DECISIONS.md` §"Single slice-model artifact (RFC-29 M2b simplification)"](../../DECISIONS.md#single-slice-model-artifact-rfc-29-m2b-simplification).

## D5 — Operator-driven `divergence`

The CLI is the single writer of every `Divergence` variant, all through `specrun plan amend --divergence`. Operators flip `accepted | rejected`; the `/spec:plan` agent stages `likely` after `specrun plan propose --from`. `plan create` scaffolds an empty plan and never stamps divergence. See [`crates/workflow/src/change/plan/core/model.rs`](../../crates/workflow/src/change/plan/core/model.rs).

## D6 — Discovery aliases

Leads carry an optional `aliases[]` bullet. `slices[].sources[].lead` resolves first against `id`, then against any entry in `aliases[]`. Aliases live in a single namespace per `discovery.md`. Parser at [`crates/model/src/discovery/document.rs`](../../crates/model/src/discovery/document.rs); collision discriminant `discovery-alias-collision`.

## D7 — `--auto-approve`

`specrun plan create --auto-approve` stamps Gate 1 in the same invocation when validation passes. Failure under `--auto-approve` MUST NOT stamp; the operator re-runs after fixing.

## D8 — Cache fingerprint inputs

Closed five-input list: source path canonicalised, adapter `name@version`, brief sha256, sorted declared-tool versions, lead id. Cache at `.specify/.cache/extractions/<adapter>/<fingerprint>/` with append-only `index.jsonl` at the adapter root. Implementation at [`crates/workflow/src/adapter/cache.rs`](../../crates/workflow/src/adapter/cache.rs); see [`DECISIONS.md` §"Extraction cache fingerprint inputs"](../../DECISIONS.md#extraction-cache-fingerprint-inputs).

## Provenance projection

Closed top-level shape on the projected view: `version`, `slice`, `generated-at`, `generator`, `requirements[]`. The view is computed from `model.yaml` on demand by `specrun slice provenance` and is never persisted. See [`crates/workflow/src/slice/provenance.rs`](../../crates/workflow/src/slice/provenance.rs) and `kind: tool` evaluator contract above.
