# Architecture

Workspace shape, crate dependency direction, the WASI carve-out, the `Layout<'a>` boundary, time injection, network hardening, and the rationale behind atomic writes. Read this before adding a new crate or shifting where state lives.

## Workspace layout

Binary crate (`name = "specify"`) at the repo root. `src/bin/specrun.rs` and `src/bin/specdev.rs` are thin `ExitCode` shims over `specify::runtime::run` and `specify::authoring::run` in [`src/lib.rs`](../../src/lib.rs); hosting dispatch in library modules keeps binary entry points minimal and supports doc tests. The `specrun` tree lives under [`src/runtime/`](../../src/runtime/); the `specdev` tree under [`src/authoring/`](../../src/authoring/). Workspace member crates live under `crates/`; the dependency direction is leaf → root:

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-schema                   # depends on specify-error (embedded JSON Schemas + jsonschema plumbing)
specify-tool                     # depends on specify-error (WASI tool runner; wasmtime, gated)
specify-lints                    # standards layer — depends on specify-{error,schema,tool}; NOT on specify-domain
specify-domain                   # workflow layer — depends on specify-{error,schema,tool}; NOT on specify-lints
specify-authoring                # depends on specify-{error,schema,codex} (framework authoring checks; publish=false)
specify (root crate)             # wires runtime + authoring crates into specrun/specdev
```

`specify-lints` and `specify-domain` are siblings: the workflow layer and the standards layer never import each other. `specify-schema` sits at the leaf layer alongside `specify-error` so both higher layers consume the embedded JSON Schemas through one crate. The Phase 1B collapse from 13 crates to the four-library shape (and the subsequent RFC-32 split that re-introduced `specify-lints` and `specify-schema`) is logged in [DECISIONS.md §"Crate layout"](../../DECISIONS.md#crate-layout) and [DECISIONS.md §"RFC-32 — Standards layer split into `specify-lints` and `specify-schema`"](../../DECISIONS.md#rfc-32--standards-layer-split-into-specify-lints-and-specify-schema); the module boundaries inside `specify-domain` preserve the prior cross-crate split.

### Standards layer vs workflow layer

`specify-lints` (standards) and `specify-domain` (workflow) are deliberately siblings. The §"Principles" / "No lifecycle authority in review" rule from [RFC-32 §"Principles"](https://github.com/augentic/specify/blob/main/rfcs/rfc-32-standards-enforcement.md#principles) is a type-system invariant rather than a coding convention: `specify-domain` MUST NOT depend on `specify-lints` (review code never reaches workflow lifecycle types), and `specify-lints` MUST NOT depend on `specify-domain` (review code cannot transition a slice or stamp a plan). Both depend on `specify-schema` so the embedded JSON Schemas live in one place. If a future workflow validator needs to mint a `LintFinding`, `specify-domain` gains a dependency on `specify-lints` at that point (leaf-→-root still holds); v1 does not need this and the sibling shape stays. Refer to [RFC-32 §"Library layout"](https://github.com/augentic/specify/blob/main/rfcs/rfc-32-standards-enforcement.md#library-layout).

Every crate uses the shared `[workspace.package]` (`edition = "2024"`, `rust-version = "1.93"`, MIT/Apache-2.0) and the shared `[workspace.lints]` block in the root `Cargo.toml` (clippy `all`/`cargo`/`nursery`/`pedantic` warned, plus a hand-picked `restriction` subset and a tightened rust lint set — `missing_debug_implementations`, `single_use_lifetimes`, `redundant_lifetimes`).

**Hard dependency rule:** `specify-error` is the leaf and depends on no other workspace crate. Adding a workspace dep to `specify-error` re-introduces the cycle the layering was designed to avoid; do not. The long-form rationale lives in [DECISIONS.md §"Error layering"](../../DECISIONS.md#error-layering).

**New workspace crates** are an exception, not the default. See [DECISIONS.md §"New workspace crates"](../../DECISIONS.md#new-workspace-crates) for the bar a new crate must clear.

The root `specify` crate exposes `src/lib.rs` (crate root), `src/runtime.rs` + `src/runtime/` (`specrun` dispatch), and `src/authoring.rs` + `src/authoring/` (`specdev` dispatch). Clap introspection for shell completions lives in [`src/runtime/commands.rs`](../../src/runtime/commands.rs) via `Cli::command()`.

## workflow domain modules

Four `specify-domain` modules carry the workflow contract; touching any of them requires a cross-repo `rg` sweep per [AGENTS.md §"When working in this repo"](../../AGENTS.md#when-working-in-this-repo).

- **`crates/domain/src/adapter/`** — axis-split loader. `SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points for loading a source or target adapter manifest; each carries its closed operation set (`SourceOperation` / `TargetOperation`) as the typed `briefs.keys()` source-of-truth, with serde rejecting unknown variants at the YAML parse boundary. The closed `Axis::{Source, Target}` enum still routes cache paths and the runtime dispatcher used by `specify {source,target} resolve`; see [DECISIONS.md §"Adapter loader axis routing"](../../DECISIONS.md#adapter-loader-axis-routing) for the long form. The legacy axis-agnostic `crate::adapter` module (with `Adapter`/`Pipeline`/`PipelineView`/`Phase`/`AdapterSource`/`ResolvedAdapter`) was retired in the F9 collapse; the 1.x `Brief`/`BriefFrontmatter` parser was retired together with `schemas/brief/schema.json` (Specify 2.0 briefs are resolved by path and the CLI never reads their bodies), and `CacheMeta` is in [`init/cache.rs`](../../crates/domain/src/init/cache.rs).
- **`crates/domain/src/spec/provenance.rs`** — parser and validator for the requirement-block provenance metadata (`ID:`, `Sources:`, `Status:`) that core synthesis emits at the top of every `spec.md` requirement. `RequirementStatus` is closed (`agreed | unknown | conflict | divergence`); the inline `[…]` tag on the requirement heading must agree with the `Status:` line. Findings aggregate so one malformed block does not mask later problems.
- **`crates/domain/src/journal.rs`** — RFC-19 newline-delimited JSON event log at `<project_dir>/.specify/journal.jsonl`. Closed `Event` / `EventKind` taxonomy; kebab-case dotted wire ids (`plan.transition.approved`, `plan.propose.divergence`, `plan.amend.divergence`, `slice.transition.refined`, `slice.extract.completed`, `slice.synthesis.{conflict,divergence,unknown}`) bridge to `snake_case` Rust variants via `#[serde(rename = "…")]`. Append is atomic and is the only mutation; readers tail the file and skip blank lines.
- **`crates/domain/src/schema.rs`** — workflow-aware validation wrappers for the on-disk workflow artifacts (`schemas/plan/plan.schema.json`, `schemas/evidence.schema.json`, the adapter/source/target manifest schemas, `schemas/discovery/candidate.schema.json`). The raw embedded schema constants and the generic `jsonschema` plumbing live in `crates/schema/` (`specify-schema`) per RFC-32 §"Library layout"; this module imports them and adds the workflow-shaped error aggregation (`Plan`-aware payloads, per-file finding rollups, the `rule_id` strings the CLI surfaces). Validators return `Error::Validation` so the CLI exits with code 2 (`Exit::ValidationFailed`); `specrun plan add` / `plan amend` / `slice validate` are the first-use hooks.

## Per-axis cache layout

`Adapter::resolve` probes — in order — the agent-populated cache at `<project_dir>/.specify/.cache/{sources,targets}/<name>/` and then the in-repo manifest at `<project_dir>/{sources,targets}/<name>/`. The `{sources,targets}` segment is keyed by `Axis`, so source and target adapters with colliding names disambiguate by axis. `cache_dir(axis, name)` returns the cache-side path. Do not collapse the two roots or special-case one axis — workflow §"Resolver and cache" pins the shape.

## WASI tool sidecar scope

The WASI tool cache root resolves `$SPECIFY_TOOLS_CACHE` → `$XDG_CACHE_HOME/specify/tools/` → `$HOME/.cache/specify/tools/`. Inside it the scope segment is `project--<project-name>` for project-scope tools and `adapter--<axis>--<slug>` for adapter-scope tools (e.g. `adapter--target--contracts` for tools declared in the `contracts` target adapter's `tools.yaml`). The `--` separator avoids collisions with hyphenated tool names. The adapter-scope substitution variable that maps into permission paths is `$CAPABILITY_DIR` (it expands to the resolved adapter's root directory and is rejected on project-scope use as `tool.capability-dir-out-of-scope`). The pre-2.0 `adapter--<slug>` scope segment and `$ADAPTER_DIR` variable were retired in Wave 0.3 — see [DECISIONS.md §"`$CAPABILITY_DIR` replaces `$ADAPTER_DIR`"](../../DECISIONS.md#capability_dir-replaces-adapter_dir).

## WASI carve-outs

WASI tools live in `wasi-tools/`, a sibling workspace excluded from the main lint posture. Members are `wasi-tools/contract` (`specify-contract`) and `wasi-tools/vectis` (`specify-vectis`). Build them by running `cargo build` inside `wasi-tools/` so the sibling workspace's lockfile and target dir are used — `cargo make contract-wasm` is a thin wrapper that does this for `specify-contract` and is required before running `tests/contract_tool.rs`; `scripts/build-vectis-local.sh` does the same for `specify-vectis` and adds sha256 sidecars for pre-release smoke tests.

`wasi-tools/contract` and `wasi-tools/vectis` are deliberate carve-outs from the workspace's Render/emit/`specify-error` discipline. They ship as standalone WASI components and live in their own sibling workspace at `wasi-tools/Cargo.toml`, which inherits a leaner lint posture and a minimal `[workspace.dependencies]` set. Do not pull `specify-error` (or any other host workspace crate that drags in `wasmtime`, `tokio`, `ureq`, …) into either; the carve-out comments in `wasi-tools/contract/src/main.rs` and `wasi-tools/vectis/src/lib.rs` are authoritative.

**Carve-out invariant.** A plugin's validation, scaffold, and rendering logic lives inside its carve-out; the host CLI consumes it only through `specrun tool run <name>`. No `specify-*` workspace crate may import plugin-specific logic — the previous shared-validation split (`specify-validate` re-extracted for the contract baseline checks) was an architectural leak collapsed in the 2026-05 inversion pass. New source / target adapters ship as carve-outs (or as in-repo brief bundles consumed by the agent) and stay there.

When editing these crates:

- They cannot use anything that isn't WASI-compatible. No threads, no networking primitives outside the declared WASI imports, no clock unless the manifest declares it.
- They stay outside the host workspace's Render/emit/`specify-error` discipline. Do not pull host workspace crates into either; the carve-out is the single source of truth for the plugin's logic.
- Rebuild artifacts from inside `wasi-tools/` so the sibling workspace's lockfile is used (`cargo make contract-wasm` and `scripts/build-vectis-local.sh` both do this). Do not check the `.wasm` outputs into git — the release workflow handles distribution.
- Keep their crate dependency surface minimal — they ship as standalone components and bloat the WASM size if you pull in heavy crates.

The `specify-tool` runner (`wasmtime` + `wasmtime-wasi`) loads them through `specrun tool run <name>` per declared-tool permissions in `project.yaml.tools[]`.

## Layout boundary

`.specify/` is framework-managed state every CLI verb writes through (configuration under `project.yaml`, `slices/`, `archive/`, `.cache/`, `workspace/`, `plans/`, `plan.lock`). Operator-facing platform artifacts (`registry.yaml`, `plan.yaml`, `change.md`, `contracts/`) live at the repo root. The boundary is enforced by the `Layout<'a>` newtype in `specify-domain` (`crates/domain/src/config.rs`): path helpers are inherent methods on `Layout<'a>`, and call sites write `Layout::new(&dir).plan_path()`. Do not hard-code `.specify/registry.yaml` or sibling paths, and do not declare free path-helper functions outside `crates/domain/src/config/`; any new `.specify/` path lands on `Layout`.

## Time injection

Functions that record a timestamp into a serialised artifact accept `now: jiff::Timestamp` from the dispatcher boundary. Library crates do not call `Timestamp::now()`; the call site lives in `src/runtime/commands/*.rs` so tests can pin time deterministically. The current carve-out — `slice_actions::*` and friends still consume an injected `now` argument — is the canonical shape to follow.

## ureq fetch hardening

The WASI tool fetch in `crates/tool/src/resolver.rs` runs every HTTP request with explicit per-call timeouts, a `MAX_RESPONSE_BYTES` cap (64 MiB) checked on both the `Content-Length` header and the streamed body, and streams the response to a tempfile before persisting into the cache. Any new HTTP path that lands in this crate must adopt the same shape (timeouts + size cap + stream-to-tempfile); do not buffer arbitrary remote bodies into memory.

## Atomic writes

Use `yaml_write` (in `crates/slice/src/atomic.rs`) for any file a concurrent reader may observe mid-write: `plan.yaml`, `.metadata.yaml`, `plan.lock`, and the registry. It serialises to `NamedTempFile::new_in(parent)` and `persist`-renames over the target so readers either see the prior bytes or the new bytes. Plain `fs::write` is reserved for files no other process reads concurrently with the writer (one-shot scratch output, fixtures inside a tempdir test).

The standards-side phrasing of the rule lives in [coding-standards.md §"YAML, JSON, and atomic writes"](./coding-standards.md#yaml-json-and-atomic-writes); the long-form rationale lives in [DECISIONS.md §"Atomic writes"](../../DECISIONS.md#atomic-writes).

## Toolchain

Rust stable per `rust-toolchain.toml` (channel `stable`, components `clippy`, `rust-src`, `rustfmt`). WASM targets pre-installed via `targets = ["aarch64-apple-darwin", "wasm32-wasip2", "x86_64-apple-darwin"]`.

`rustfmt.toml` uses unstable nightly features (`unstable_features = true`, `imports_granularity = "Module"`, `group_imports = "StdExternalCrate"`). Format with nightly:

```bash
cargo +nightly fmt --all
```

`cargo make fmt` does this for you.

## Supply chain

`cargo-vet`, `cargo-deny`, `cargo-audit`, `cargo-outdated`, and `cargo-udeps` all run in CI (`cargo make ci`). When a new dependency lands:

1. Add it to `[workspace.dependencies]` in the root `Cargo.toml` with a major-version pin (e.g. `serde = { version = "1", features = ["derive"] }`). Per-crate `Cargo.toml` references it as `serde.workspace = true`.
2. Run `cargo make vet` to regenerate the supply-chain audits, then commit the diff.
3. Check `deny.toml` allows the dependency's licence. The current allowlist is in `deny.toml`; add a new SPDX id only after confirming compatibility with MIT-OR-Apache-2.0.

`clippy::multiple_crate_versions` is silenced workspace-wide (`Cargo.toml`'s `[workspace.lints.clippy]`); duplicate transitive versions are audited by hand via `cargo tree --duplicates` on each `cargo update`, not gated through a ratchet.

## Skill / CLI responsibility split

Every deterministic operation lives in this CLI: kebab-case validation, `.metadata.yaml` reads/writes, lifecycle transitions, plugin resolution (`specrun source resolve` / `specrun target resolve`), artifact-completion checks, spec-merge preview, baseline conflict detection, delta merge, coherence validation, archive moves, plan/registry validation, schema validation of `plan.yaml` and per-source `Evidence`, RFC-19 journal append. The plugin repo's `/spec:` skills (`/spec:plan`, `/spec:refine`, `/spec:build`, `/spec:merge`, `/spec:execute`, `/spec:finalize`, `/spec:init`, `/spec:drop`) shell out for all of those.

The corollary: when a skill currently does something deterministic in prose (parsing YAML, validating shape, computing topology, transitioning state), the right fix is to add a CLI verb here and have the skill call it. The wrong fix is to make the skill smarter.

The parent repo's [`AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) is the source of truth for workflow vocabulary (slice / change), skill family, plan-driven loop, and contract skills.
