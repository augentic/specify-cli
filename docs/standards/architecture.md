# Architecture

Workspace shape, crate dependency direction, the WASI carve-out, the `Layout<'a>` boundary, time injection, network hardening, and the rationale behind atomic writes. Read this before adding a new crate or shifting where state lives.

## Workspace layout

Binary crate (`name = "specify"`) at the repo root. `src/bin/specrun.rs` and `src/bin/specdev.rs` are thin `ExitCode` shims over `specify::runtime::run` and `specify::authoring::run` in [`src/lib.rs`](../../src/lib.rs); hosting dispatch in library modules keeps binary entry points minimal and supports doc tests. The `specrun` tree lives under [`src/runtime/`](../../src/runtime/); the `specdev` tree under [`src/authoring/`](../../src/authoring/). Workspace member crates live under `crates/`; the dependency direction is leaf → root:

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-digest                   # leaf — sha2 + base16ct only (SHA-256 hex digest encoding)
specify-schema                   # depends on specify-error (embedded JSON Schemas + jsonschema plumbing)
specify-diagnostics              # depends on specify-{error,schema,digest} (Diagnostic substrate: report, fingerprint, validator, renderers, blocking)
specify-model                    # depends on specify-{error,diagnostics} (artifact types + parsers: spec, task, evidence, discovery; shared atomic writer)
specify-tool                     # depends on specify-{error,diagnostics,digest} (WASI tool runner; wasmtime, gated)
specify-validate                 # depends on specify-{model,error,diagnostics} — artifact rule registry; NOT on specify-workflow or anything named lint
specify-standards                # standards layer — depends on specify-{error,schema,digest,diagnostics}; NOT on specify-workflow or specify-tool
specify-workflow                 # workflow layer — depends on specify-{error,schema,digest,tool,model,diagnostics}; NOT on specify-standards / specify-validate
specify (root crate)             # wires runtime + framework crates into specrun/specdev
```

The framework authoring checks behind `specdev lint` are no longer a standalone crate: the dissolved `specify-authoring` now lives as the `specify_standards::framework` module (see [DECISIONS.md §"Crate layout"](../../DECISIONS.md#crate-layout)).

`specify-standards` (standards) and `specify-workflow` (workflow) are siblings: they never import each other. `specify-validate` is the validation analog: it depends on `specify-model` only and never on `specify-workflow`, so an artifact rule cannot reach workflow lifecycle types — the same no-lifecycle-authority invariant `specify-standards` enforces. `specify-model` is the lifecycle-free leaf carrying the artifact types and parsers both `specify-validate` and `specify-workflow` read, alongside `specify-schema` and `specify-error` at the bottom. The Phase 1B collapse from 13 crates, the standards-layer split that re-introduced `specify-standards` and `specify-schema`, and the model/validate split that extracted `specify-model` and `specify-validate` are logged in [DECISIONS.md §"Crate layout"](../../DECISIONS.md#crate-layout) and [DECISIONS.md §"Standards layer split into `specify-standards` and `specify-schema`"](../../DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema).

### Standards layer vs workflow layer

`specify-standards` (standards) and `specify-workflow` (workflow) are deliberately siblings. The §"Principles" / "No lifecycle authority in review" rule from [DECISIONS.md §"Standards layer split into `specify-standards` and `specify-schema`"](../../DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema) is a type-system invariant rather than a coding convention: `specify-workflow` MUST NOT depend on `specify-standards` (review code never reaches workflow lifecycle types), and `specify-standards` MUST NOT depend on `specify-workflow` (review code cannot transition a slice or stamp a plan). Both depend on `specify-schema` so the embedded JSON Schemas live in one place, and both depend on the `specify-diagnostics` leaf for the neutral `Diagnostic` substrate — so a workflow validator mints findings without `specify-workflow` ever depending on anything named `lint`. See [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](../../DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate). Refer to [DECISIONS.md §"Standards layer split into `specify-standards` and `specify-schema`"](../../DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema).

Every crate uses the shared `[workspace.package]` (`edition = "2024"`, `rust-version = "1.93"`, MIT/Apache-2.0) and the shared `[workspace.lints]` block in the root `Cargo.toml` (clippy `all`/`cargo`/`nursery`/`pedantic` warned, plus a hand-picked `restriction` subset and a tightened rust lint set — `missing_debug_implementations`, `single_use_lifetimes`, `redundant_lifetimes`).

**Hard dependency rule:** `specify-error` is the leaf and depends on no other workspace crate. Adding a workspace dep to `specify-error` re-introduces the cycle the layering was designed to avoid; do not. The long-form rationale lives in [DECISIONS.md §"Error layering"](../../DECISIONS.md#error-layering).

**New workspace crates** are an exception, not the default. See [DECISIONS.md §"New workspace crates"](../../DECISIONS.md#new-workspace-crates) for the bar a new crate must clear.

The root `specify` crate exposes `src/lib.rs` (crate root), `src/runtime.rs` + `src/runtime/` (`specrun` dispatch), and `src/authoring.rs` + `src/authoring/` (`specdev` dispatch). Clap introspection for shell completions lives in [`src/runtime/commands.rs`](../../src/runtime/commands.rs) via `Cli::command()`.

## standards layer modules

Three `specify-standards` module trees carry the standards-layer contract; touching any of them requires a cross-repo `rg` sweep per [AGENTS.md §"When working in this repo"](../../AGENTS.md#when-working-in-this-repo).

- **`crates/standards/src/rules/`** — rules parser and resolver pipeline (`parse.rs`, `resolve.rs`, `resolve/{filter,sort}.rs`). The fingerprint algorithm and finding validators live in the `specify-diagnostics` leaf — import them from there directly. The resolver walks both `adapters/shared/rules/universal/` (`Origin::Shared`) and `adapters/shared/rules/core/` (`Origin::Core`) and tags every resolved rule with its origin so `specrun lint` / `specrun rules export` can default-exclude `CORE-*` unless `--include-core` is passed (§A3).
- **`crates/standards/src/lint/index/`** — dual-profile indexer that produces a `WorkspaceModel` from a tree on disk. The closed `ScanProfile::{Consumer, Framework}` enum picks the walk shape: the consumer profile roots at `project_dir` (or the supplied `artifact_paths`), records symlinks without traversing them, and runs only the shared per-file extractors (`frontmatter`, `markdown`, `ignore_directives`); the framework profile (`index/framework.rs`) applies the §F1 include set, follows symlinks with cycle detection (recording both endpoints), and runs the framework-only extractors (`skill.rs`, `adapter.rs`, `marketplace.rs`, `agent_teams.rs`, `brief.rs`) alongside the shared passes. `lint::index::build(project_dir, ScanProfile::Framework, &[], &[])` is the entry point `specdev lint` calls; `specrun lint` calls the consumer counterpart. Both profiles share the same `WorkspaceModel` assembly invariants (byte-stable enumeration, sorted output collections).
- **`crates/standards/src/lint/eval/`** — deterministic-hint interpreters for the landed Phase 2 kinds (`path_pattern.rs`, `regex.rs`, `schema.rs`, `tool.rs`); reserved kinds in `schemas/rules/rule.schema.json` (annotated `"x-hint-status": "reserved"`) land paired with their interpreter implementation, one PR per kind. `lint-mode: model-assisted` rules are not skipped — they surface as `kind: review` diagnostics (the deterministic engine raises the question without scoring it). The four formatters live in the neutral `specify-diagnostics` leaf (`crates/diagnostics/src/render/{json,pretty,github,compact}.rs`) and consume the closed `Diagnostic` shape every surface emits.

## workflow domain modules

Four module trees carry the workflow contract — three in `specify-workflow`, plus `spec/provenance.rs` which now lives in `specify-model`; touching any of them requires a cross-repo `rg` sweep per [AGENTS.md §"When working in this repo"](../../AGENTS.md#when-working-in-this-repo).

- **`crates/workflow/src/adapter/`** — axis-split loader. `SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points for loading a source or target adapter manifest; each carries its closed operation set (`SourceOperation` / `TargetOperation`) as the typed `briefs.keys()` source-of-truth, with serde rejecting unknown variants at the YAML parse boundary. The closed `Axis::{Source, Target}` enum still routes cache paths and the runtime dispatcher used by `specify {source,target} resolve`; see [DECISIONS.md §"Adapter loader axis routing"](../../DECISIONS.md#adapter-loader-axis-routing) for the long form. The legacy axis-agnostic `crate::adapter` module (with `Adapter`/`Pipeline`/`PipelineView`/`Phase`/`AdapterSource`/`ResolvedAdapter`) was retired in the F9 collapse; the 1.x `Brief`/`BriefFrontmatter` parser was retired together with `schemas/brief/schema.json` (Specify 2.0 briefs are resolved by path and the CLI never reads their bodies), and `CacheMeta` is in [`init/cache.rs`](../../crates/workflow/src/init/cache.rs).
- **`crates/model/src/spec/provenance.rs`** — parser and validator for the requirement-block provenance metadata (`ID:`, `Sources:`, `Status:`) that core synthesis emits at the top of every `spec.md` requirement. `RequirementStatus` is closed (`agreed | unknown | conflict | divergence`); the inline `[…]` tag on the requirement heading must agree with the `Status:` line. Findings aggregate so one malformed block does not mask later problems.
- **`crates/workflow/src/journal.rs`** — newline-delimited JSON journal event log at `<project_dir>/.specify/journal.jsonl`. Closed `Event` / `EventKind` taxonomy; kebab-case dotted wire ids (`plan.transition.approved`, `plan.propose.divergence`, `plan.amend.divergence`, `slice.transition.refined`, `slice.extract.completed`, `slice.synthesis.{conflict,divergence,unknown}`) bridge to `snake_case` Rust variants via `#[serde(rename = "…")]`. Append is atomic and is the only mutation; readers tail the file and skip blank lines.
- **`crates/workflow/src/schema.rs`** — workflow-aware validation wrappers for the on-disk workflow artifacts (`schemas/plan/plan.schema.json`, `schemas/evidence.schema.json`, the adapter/source/target manifest schemas, `schemas/discovery/lead.schema.json`). The raw embedded schema constants and the generic `jsonschema` plumbing live in `crates/schema/` (`specify-schema`) per [DECISIONS.md §"Standards layer split into `specify-standards` and `specify-schema"](../../DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema); this module imports them and adds the workflow-shaped error aggregation (the `rule_id` strings the CLI surfaces, joined into the payload-free error `detail`). Validators return the payload-free `Error::Validation { code, detail }` so the CLI exits with code 2 (`Exit::ValidationFailed`) with the specific discriminant as the wire `error`; surfaces that render findings (`slice validate`) emit a `DiagnosticReport` on stdout first. `specrun plan add` / `plan amend` / `slice validate` are the first-use hooks. See [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](../../DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate).

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

`.specify/` is framework-managed state every CLI verb writes through (configuration under `project.yaml`, `slices/`, `archive/`, `.cache/`, `workspace/`, `plans/`, `plan.lock`). Operator-facing platform artifacts (`registry.yaml`, `plan.yaml`, `change.md`, `contracts/`) live at the repo root. The boundary is enforced by the `Layout<'a>` newtype in `specify-workflow` (`crates/workflow/src/config.rs`): path helpers are inherent methods on `Layout<'a>`, and call sites write `Layout::new(&dir).plan_path()`. Do not hard-code `.specify/registry.yaml` or sibling paths, and do not declare free path-helper functions outside `crates/workflow/src/config/`; any new `.specify/` path lands on `Layout`.

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

Every deterministic operation lives in this CLI: kebab-case validation, `.metadata.yaml` reads/writes, lifecycle transitions, plugin resolution (`specrun source resolve` / `specrun target resolve`), artifact-completion checks, spec-merge preview, baseline conflict detection, delta merge, coherence validation, archive moves, plan/registry validation, schema validation of `plan.yaml` and per-source `Evidence`, journal event append. The plugin repo's `/spec:` skills (`/spec:plan`, `/spec:refine`, `/spec:build`, `/spec:merge`, `/spec:execute`, `/spec:finalize`, `/spec:init`, `/spec:drop`) shell out for all of those.

The corollary: when a skill currently does something deterministic in prose (parsing YAML, validating shape, computing topology, transitioning state), the right fix is to add a CLI verb here and have the skill call it. The wrong fix is to make the skill smarter.

The parent repo's [`AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) is the source of truth for workflow vocabulary (slice / change), skill family, plan-driven loop, and contract skills.
