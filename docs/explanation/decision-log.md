# Decision archaeology

Frozen post-mortem material extracted from [`DECISIONS.md`](../../DECISIONS.md). Kept for archaeological context — none of these notes describe a standing rule; the rules they led to live in `DECISIONS.md`, `AGENTS.md`, or `docs/standards/`.

## Diag-first error policy — historical variant collapse

Twelve historical one-site variants collapsed to `Error::Diag` under the diag-first policy with their kebab discriminants preserved:

- `RegistryMissing`
- `PlanNotFound`
- `PlanStructural`
- `CompatibilityCheckFailed`
- `ContextDriftDetected`
- `ContextWouldUpdate`
- `ContextNoLock`
- `ContextMissing`
- `ContextUnfenced`
- `ContextDrift`
- `InitNeedsAdapter`
- `WorkspacePushFailed`

The current rule lives in [`DECISIONS.md` §"Diag-first error policy"](../../DECISIONS.md#diag-first-error-policy); this list documents the one-time migration that produced the steady state.

## RFC-25 type rename — Wave 0 / F9 collapse history

Wave 0.2 (`cli/W0.2`) renamed `Adapter*` → `Target*` for the output-role domain types (`Target`, the `Slice.target` field, the `slice-create-target-missing` / `plan.entry-needs-project-or-target` error discriminants, plus every fixture, JSON envelope, and call site). Specify 2.0 then settled the regular/hub init guard on its documented spelling `init-requires-adapter-or-hub` (the Wave 0.2-era `init-requires-target-or-workspace` is gone).

Wave 0.3 (`cli/W0.3`) moved the shared manifest *shape* into a new axis-aware loader. The F9 collapse then retired the legacy axis-agnostic `crate::adapter` module (`Adapter` / `Pipeline` / `PipelineEntry` / `PipelineView` / `Phase` / `AdapterSource` / `ResolvedAdapter` / the legacy `adapter.schema.json`) and renamed the axis-aware loader back into `crates/domain/src/adapter/` under the cleaner `SourceAdapter` / `TargetAdapter` / `Axis` / `ResolvedAdapter` / `AdapterLocation` type names.

The 1.x brief-frontmatter parser (`Brief` / `BriefFrontmatter` with `id` / `description` / `needs` / `tracks` / `generates` fields) was retired together with the `schemas/brief/schema.json` schema: Specify 2.0 briefs are resolved by path through `briefs.<op>` on the adapter manifest and the CLI never reads their bodies, so the parser was dead code. The plugin repo's [`docs/standards/skill-authoring.md`](https://github.com/augentic/specify/blob/main/docs/standards/skill-authoring.md) §"Brief authoring" now requires briefs carry no YAML frontmatter at all.

`CacheMeta` was rehomed inside [`crates/domain/src/init/cache.rs`](../../crates/domain/src/init/cache.rs). The `Phase` enum was replaced by `Operation { Shape, Build, Merge }` on the slice-metadata wire (`phase: shape | build | merge`).

The current rule lives in [`DECISIONS.md` §"RFC-25 type rename: `Target*` is the output role, `Adapter` is the shared shape"](../../DECISIONS.md#rfc-25-type-rename-target-is-the-output-role-adapter-is-the-shared-shape).

## Crate layout — Phase 1B / `specify-validate` carve-out history

Until Phase 1B of the 2026-05 cleanup the workspace had 13 crates; the fragmentation cost more than it earned (wide build graph, redundant `Cargo.toml` files, indirect re-export hops, repeated duplicate-version exemptions). The collapse to four crates (`specify-error`, `specify-tool`, `specify-domain`, `specify`) preserved the original module boundaries; `pub` cross-module surfaces match the prior cross-crate `pub use` exports.

`specify-validate` was a Phase 1B re-extraction that owned the baseline-contract validation primitives (`ContractFinding`, `validate_baseline`) and was shared between `specify-domain` and the `wasi-tools/contract` carve-out. The 2026-05 architecture-inversion pass collapsed it into the carve-out: an adapter's validation logic belongs inside its WASI tool, not as a `specify-*` workspace crate the host can link against. Operators run `specrun tool run contract -- "$PWD/contracts"` as a pre-flight when they need that gate, identical to every other adapter. The carve-out is now self-contained; `wasi-tools/Cargo.toml` no longer has a path bridge into the host workspace.

The current rule lives in [`DECISIONS.md` §"Crate layout"](../../DECISIONS.md#crate-layout).
