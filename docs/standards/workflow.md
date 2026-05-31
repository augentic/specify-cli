# Workflow contract

The in-force contract this binary implements. Stable anchors that source code and skill briefs cite by `§`-name. This document is the live anchor surface for workflow behavior.

## Adapter vocabulary

Two adapter roles — `source` (operations: `survey`, `extract`) and `target` (operations: `shape`, `build`, `merge`). The shared on-disk shape is `adapter.yaml`; per-axis schemas refine it. See the parent repo's [`AGENTS.md` §"Vocabulary"](https://github.com/augentic/specify/blob/main/AGENTS.md#vocabulary).

## Adapter implementation shape

Per-adapter `adapter.yaml` carries `name`, `version`, `axis`, the required closed `execution` mode (`agent` | `tool`, RFC-29 D9), `description`, the `briefs.<operation>` map, an optional `cache` opt-out, and an optional `tools[]` array. The closed operation set is determined by the manifest's `axis`. `execution: agent` forces `cache: opt-out`; the loader rejects a manifest that omits `execution` (`adapter-execution-mode-required`) or declares `execution: agent` alongside a non-opt-out cache mode (`adapter-execution-agent-cache-conflict`). Implementation: [`crates/workflow/src/adapter/`](../../crates/workflow/src/adapter); per-axis schemas at [`schemas/adapter.schema.json`](../../schemas/adapter.schema.json), [`source.schema.json`](../../schemas/source.schema.json), [`target.schema.json`](../../schemas/target.schema.json).

## Source adapter contract

`axis: source`; `briefs.keys() ⊆ {extract, survey}`. `survey` writes `## Lead inventory` blocks under `discovery.md` at plan time; `extract` writes one Evidence document per `(source-key, lead-id)` pair at slice time. See [`schemas/source.schema.json`](../../schemas/source.schema.json) and [`schemas/evidence.schema.json`](../../schemas/evidence.schema.json).

`specrun source survey <source-key> [--plan <name>] [--phase prepare|finalize]` and `specrun source extract <source-key> <lead-id> --slice <slice> [--phase prepare|finalize]` are the CLI-owned runners (RFC-29 D1). `<source-key>` resolves against `plan.yaml.sources.<key>`, then the adapter from `SourceBinding.adapter`. Both validate before the write becomes visible (lead set against `schemas/discovery/lead.schema.json` then `discovery.md` merge; Evidence against `schemas/evidence.schema.json` then persist to `.specify/slices/<slice>/evidence/<source-key>.yaml`). Under `execution: agent` dispatch is two-phase (`prepare` builds the sandbox + prints the handoff envelope; `finalize` validates / persists / caches / journals); under `execution: tool` a single call runs the whole operation. Value-bound sources (`intent`) carry `value-inline`; path bindings carry `source-path`. See [`DECISIONS.md` §"Source operations (D1)"](../../DECISIONS.md#source-operations-d1) and [`DECISIONS.md` §"Adapter execution mode (D9)"](../../DECISIONS.md#adapter-execution-mode-d9).

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

`survey` writes `## Lead inventory` blocks — one **raw, unmerged** lead per source, each identified by its `(source-key, lead-id)` pair (`survey` stamps `source-key` from the surveyed source). A re-survey of one source replaces only that source's blocks by `(source-key, lead-id)`; the same `lead-id` may appear under different source keys, and the lead-id/alias namespace is scoped per `source-key`. The operator stamps `approved`; `extract` resolves `slices[].sources[].lead-id` against `lead-id`-then-`aliases[]` within the binding's `source-key`. Cross-source unification is deferred to plan-time reconciliation (D2). Schema at [`schemas/discovery/lead.schema.json`](../../schemas/discovery/lead.schema.json); parser at [`crates/model/src/discovery/document.rs`](../../crates/model/src/discovery/document.rs).

## The Plan

`plan.yaml` shape is fixed by [`schemas/plan/plan.schema.json`](../../schemas/plan/plan.schema.json). Two stored lifecycle states (`pending | approved`); per-entry status is `pending | in-progress | done`. Writer ownership is split — see §"Writer ownership" below.

## Workflow vocabulary

`Slice`, `Lead`, `Evidence`, `Source`, `Target`, `Plan`, `Discovery`. Definitions live in the parent repo's [`AGENTS.md` §"Vocabulary"](https://github.com/augentic/specify/blob/main/AGENTS.md#vocabulary).

## Plan-time reconciliation

`specrun plan propose` reconciles surveyed leads across sources at plan time and writes the `plan.yaml.slices[]` rows (RFC-29 D2). `--dry-run [--format json]` reads `plan.yaml.sources`, the `discovery.md` lead inventory, and the project topology, then emits the `kind: request` envelope (flat `(source-key, lead-id)` lead catalog + `projects[]`) for the agent; it writes nothing. `--from <response.json> [--format json]` is the only slice writer: it schema-gates the response (`PROPOSAL_JSON_SCHEMA` at [`schemas/discovery/proposal.schema.json`](../../schemas/discovery/proposal.schema.json), kebab wire fields, closed `kind: request | response`), re-reads `discovery.md`, validates the partition over scopes (total coverage, at most one lead per source, fan-out `sources[]` consistency), derives slice names, binds each slice's `project` and `target`, and replaces `slices[]` only on a replaceable plan (`lifecycle: pending` and every entry `pending`). Cross-source matching is agent judgment; the operator curates at Gate 1. The closed `plan-reconcile-*` / `plan-propose-mode-required` codes are `Error::Validation` outcomes (exit 2). See [`DECISIONS.md` §"Lead reconciliation (D2)"](../../DECISIONS.md#lead-reconciliation-d2) and [`crates/workflow/src/change/plan/core/propose.rs`](../../crates/workflow/src/change/plan/core/propose.rs).

The closed `Divergence` enum (`none | likely | accepted | rejected`) records a reconciliation outcome's confidence. See [`DECISIONS.md` §"`Divergence` enum"](../../DECISIONS.md#divergence-enum) and [`crates/workflow/src/change/plan/core/model.rs`](../../crates/workflow/src/change/plan/core/model.rs).

## Source

`plan.yaml.sources.<key>` is the structured `{ adapter, path?, value? }` object with exactly one of `path` / `value`. See [`DECISIONS.md` §"Plan source bindings"](../../DECISIONS.md#plan-source-bindings).

`Slice.sources` (a slice's per-source binding list) accepts the bare-string shorthand on parse and serialises as the structured `{ source-key, lead-id }`. See [`DECISIONS.md` §"`SliceSourceBinding`: bare shorthand plus structured form"](../../DECISIONS.md#slicesourcebinding-bare-shorthand-plus-structured-form).

## Authority hierarchy

Closed enum `intent > documentation > behaviour`. Resolution order: per-slice override (D3) → per-Evidence per-kind override (D2) → Evidence document-level `authority:` → conflict. Implementation: [`crates/model/src/evidence/authority.rs`](../../crates/model/src/evidence/authority.rs).

## Execution model

`pending → approved` plan-level (Gate 1; operator-only). Per-entry: `pending → in-progress → done`. `done` is absorbing in v1; the operator-reversed flow lives behind `specrun plan transition --undo` (added Phase 6 — see [`DECISIONS.md` §"Plan lifecycle: two stored states"](../../DECISIONS.md#plan-lifecycle-two-stored-states)).

## Refinement

`/spec:refine` runs `extract` per bound source, synthesizes `proposal.md` / `spec.md` / `design.md` / `tasks.md`, writes `provenance.yaml`, and transitions the slice to `refined`. Validators live in [`crates/validate/src/`](../../crates/validate/src/) and [`src/runtime/commands/slice/validate.rs`](../../src/runtime/commands/slice/validate.rs).

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

Headline verbs: `init`, `source {resolve, survey, extract, preview}`, `target resolve`, `slice {create, transition, validate, merge}`, `plan {create, propose, add, amend, transition, next, finalize}`, `workspace {sync, push, prepare}`, `tool run`, `journal emit`. See [`specify --help`](../init.md) and the parent repo's [`AGENTS.md` §"Skill / CLI responsibility split"](https://github.com/augentic/specify/blob/main/AGENTS.md#skill--cli-responsibility-split).

## Writer ownership

Per-entry status writes route to exactly one CLI verb each — `plan add` / `plan amend` write `pending`, `plan next` writes `in-progress`, `slice merge` (via `plan transition <entry> done`) writes `done`. Plan-level `approved` is operator-only. See [`DECISIONS.md` §"Lifecycle write-ownership"](../../DECISIONS.md#lifecycle-write-ownership).

## Observability

Newline-delimited JSON journal at `.specify/journal.jsonl`. The closed `EventKind` taxonomy lives in [`crates/workflow/src/journal.rs`](../../crates/workflow/src/journal.rs); the per-event table is in [`DECISIONS.md` §"Journal event names"](../../DECISIONS.md#journal-event-names). Source operations add `source.survey.cache-hit` / `.cache-miss` and `source.execution.agent`. `specrun plan propose --from` emits `plan.reconcile.agent` then `plan.reconcile.completed` atomically in one batched append on a successful write. Agent-orchestrated phases that lack a deterministic emit command write through `specrun journal emit <event-id> [--payload <json>]` — a guarded front door onto the same closed taxonomy, errors `journal-emit-unknown-event` / `journal-emit-payload-schema` (exit 2). See [`DECISIONS.md` §"`specrun journal emit` — guarded front door (D12)"](../../DECISIONS.md#specrun-journal-emit--guarded-front-door-d12).

## Operations typed at parse boundary

`briefs.keys()` is the canonical operation iterator; the closed `SourceOperation` / `TargetOperation` enums in [`crates/workflow/src/adapter/operation.rs`](../../crates/workflow/src/adapter/operation.rs) are the typed key set. See [`DECISIONS.md` §"Operations typed at parse boundary"](../../DECISIONS.md#operations-typed-at-parse-boundary).

## What was cut and why

`specify adapter *` and `specify change *` retired at 2.0; no compatibility aliases. The 1.x bare-string `sources` shorthand on `plan.yaml.sources.<key>` is gone. See [`DECISIONS.md` §"Plan source bindings"](../../DECISIONS.md#plan-source-bindings).

## Note to the implementing agent

Touching `Slice.target`, `SliceSourceBinding`, `Divergence`, `crates/model/src/spec/provenance.rs`, `crates/workflow/src/adapter/`, `crates/workflow/src/journal.rs`, `crates/workflow/src/schema.rs`, the `$CAPABILITY_DIR` env var, or the `plugin--<axis>--<slug>` tool cache scope requires a cross-repo `rg` sweep against both [`augentic/specify-cli`](https://github.com/augentic/specify-cli) and [`augentic/specify`](https://github.com/augentic/specify) in the same PR — the contract spans both repos.

## D1 — Runtime source adapter (`captures`)

`captures` emits `kind: example` Evidence claims with `replay-digest: sha256:…` anchors and default `authority: behaviour`. Schema entry in [`schemas/evidence.schema.json`](../../schemas/evidence.schema.json); claim type at [`crates/model/src/evidence/claim/example.rs`](../../crates/model/src/evidence/claim/example.rs).

## D2 — Per-kind authority on Evidence

`evidence.schema.json` carries an optional `authority-overrides` map keyed by claim kind. Synthesis consults this map before the document-level `authority:`. See [`DECISIONS.md` §"Evidence per-kind authority overrides"](../../DECISIONS.md#evidence-per-kind-authority-overrides) and [`crates/model/src/evidence/authority.rs`](../../crates/model/src/evidence/authority.rs).

## D3 — Per-slice authority on `plan.yaml`

`plan.yaml.slices[].authority-override` maps claim kind to a source key bound on the slice. Orphan keys surface as `slice-authority-override-orphan-source-key`. See [`DECISIONS.md` §"Plan per-slice authority overrides"](../../DECISIONS.md#plan-per-slice-authority-overrides).

## D4 — `provenance.yaml` is audit-only

Provenance index at `.specify/slices/<slice>/provenance.yaml`; `spec.md` is the authoritative artifact. Schema at [`schemas/slice/provenance.schema.json`](../../schemas/slice/provenance.schema.json); validator at [`crates/workflow/src/slice/provenance.rs`](../../crates/workflow/src/slice/provenance.rs). See [`DECISIONS.md` §"`provenance.yaml` audit index"](../../DECISIONS.md#provenanceyaml-audit-index).

## D5 — Operator-driven `divergence`

The CLI is the single writer of every `Divergence` variant, all through `specrun plan amend --divergence`. Operators flip `accepted | rejected`; the `/spec:plan` agent stages `likely` after `specrun plan propose --from`. `plan create` scaffolds an empty plan and never stamps divergence. See [`crates/workflow/src/change/plan/core/model.rs`](../../crates/workflow/src/change/plan/core/model.rs).

## D6 — Discovery aliases

Leads carry an optional `aliases[]` bullet. `slices[].sources[].lead` resolves first against `id`, then against any entry in `aliases[]`. Aliases live in a single namespace per `discovery.md`. Parser at [`crates/model/src/discovery/document.rs`](../../crates/model/src/discovery/document.rs); collision discriminant `discovery-alias-collision`.

## D7 — `--auto-approve`

`specrun plan create --auto-approve` stamps Gate 1 in the same invocation when validation passes. Failure under `--auto-approve` MUST NOT stamp; the operator re-runs after fixing.

## D8 — Cache fingerprint inputs

Closed five-input list: source path canonicalised, adapter `name@version`, brief sha256, sorted declared-tool versions, lead id. Cache at `.specify/.cache/extractions/<adapter>/<fingerprint>/` with append-only `index.jsonl` at the adapter root. Implementation at [`crates/workflow/src/adapter/cache.rs`](../../crates/workflow/src/adapter/cache.rs); see [`DECISIONS.md` §"Extraction cache fingerprint inputs"](../../DECISIONS.md#extraction-cache-fingerprint-inputs).

## Provenance index

Closed top-level shape on `provenance.yaml`: `version`, `slice`, `generated-at`, `generator`, `requirements[]`. See [`crates/workflow/src/slice/provenance.rs`](../../crates/workflow/src/slice/provenance.rs) and `kind: tool` evaluator contract above.
