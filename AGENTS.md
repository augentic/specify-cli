# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specify` runtime binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives those workflows compose.

## Crate graph

The workspace is leaf → root. `specify-error` and `specify-digest` are dependency leaves and depend on no other workspace crate.

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-digest                   # leaf — sha2 + base16ct only (SHA-256 hex digest encoding)
specify-schema                   # depends on specify-error (embedded JSON Schemas + jsonschema plumbing)
specify-diagnostics              # depends on specify-{error,schema,digest} (Diagnostic substrate: report, fingerprint, validator, renderers, blocking)
specify-model                    # depends on specify-{error,diagnostics} (artifact types + parsers: spec, task, evidence, discovery; shared atomic writer)
specify-agents                   # depends on specify-{error,digest,model} (init-time AGENTS.md context-fence generation: detect, render, fences, fingerprint, lock); Ctx-free, consumed only by the root binary
specify-tool                     # depends on specify-{error,diagnostics,digest,schema} (WASI tool runner; wasmtime, gated)
specify-validate                 # depends on specify-{model,error,diagnostics} — artifact rule registry; NOT on specify-workflow or anything named lint
specify-standards                # standards layer — depends on specify-{error,schema,digest,diagnostics}; NOT on specify-workflow or specify-tool
specify-workflow                 # workflow layer — depends on specify-{error,schema,digest,tool,model,diagnostics}; NOT on specify-standards / specify-validate
specify (root crate)             # wires every workspace crate above into the CLI binary
```

`specify-standards` and `specify-workflow` are siblings: neither imports the other. The standards-layer-vs-workflow-layer split is a type-system invariant by the dependency-direction invariant in [DECISIONS.md §"Standards layer split into `specify-standards` and `specify-schema"](./DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema) (lint carries no lifecycle authority). `specify-validate` mirrors that invariant for artifact validation: it depends on `specify-model` only, never on `specify-workflow`, so a rule cannot transition a slice or stamp a plan. `specify-model` is the lifecycle-free leaf holding the artifact types and parsers both higher layers read. Both standards and workflow depend on `specify-schema` so the embedded JSON Schemas live in one place. The neutral `Diagnostic` / `DiagnosticReport` substrate lives in `specify-diagnostics` (depends on `specify-{error,schema,digest}`), so every check producer — validate and lint alike — emits the same finding currency without `specify-validate` (or any non-lint producer) depending on anything named `lint`. See [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](./DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate).

Modules of note across the workspace (workflow + standards layers):

- `crates/workflow/src/platform.rs` — closed `Platform` enum (`Core | Ios | Android | Web | Desktop`, `#[serde(rename_all = "kebab-case")]`) representing the set of target platforms a project may declare in `project.yaml`. `Core` is mandatory in every set. `Ios` and `Android` have scaffold/build/verify support; `Web` and `Desktop` are type-system placeholders for future functionality. Includes `Display`, `FromStr`, and `parse_platforms_csv` for the `--platforms` CLI flag.
- `crates/workflow/src/adapter/` — axis-split adapter loader. `SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points and the only manifest loaders after the F9 collapse + workflow §"Operations typed at parse boundary" split (the legacy axis-generic `Adapter::resolve(axis, …)` shape and `PipelineView` were retired). `locate_axis` probes the manifest cache, then `<project_dir>/adapters/{sources,targets}/<name>/`, then (last) `$SPECIFY_FRAMEWORK_ROOT/adapters/{sources,targets}/<name>/` — the offline/dev/acceptance fallback that lets a disposable project resolve first-party adapters without a vendored `adapters/` tree (skipped when the env var is unset). The closed `SourceOperation` / `TargetOperation` enums in `adapter/operation.rs` are the typed `briefs.keys()` carried by each manifest struct. `TargetAdapter` also carries an optional `PlatformsCapability` (`{ required, allowed, default }`) declaring which platforms the target supports; vectis declares `required: true`.
- `crates/workflow/src/init/adapter_uri.rs` — `specify init <adapter>` argument parser. Recognises first-party **shorthand** (`omnia`, `omnia@v1`; ref defaults to `v1`) and expands it to a `$SPECIFY_FRAMEWORK_ROOT/adapters/targets/<name>/` checkout when present, else the canonical `https://github.com/augentic/specify/adapters/targets/<name>@<ref>` URL — alongside the existing local-path and GitHub-URL forms. See [`DECISIONS.md` §"First-party `<adapter>` shorthand at init"](./DECISIONS.md#first-party-adapter-shorthand-at-init).
- `crates/model/src/spec/provenance.rs` — `spec.md` requirement-block parser (`ID:` / `Sources:` / `Status:` lines, closed `RequirementStatus` enum, inline `[…]` tag coherence).
- `crates/workflow/src/change/plan/core/propose.rs` — plan-time lead-reconciliation kernel. Envelope DTOs (closed `kind: request | response`), the pure `build_request` / `build_catalog` / `resolve_topology` assembly, and the `Plan::propose_from` projection kernel behind `specify plan propose --dry-run | --from`. Also owns `Plan::reconcile_platforms` — the deterministic post-write pass that detects declared-but-absent shell platforms (via filesystem heuristics in `detect_missing_platforms`) and inserts bootstrap slices (`app-foundation` for greenfield, `bootstrap-<platform>` for incremental), triggered by `--reconcile-platforms` on `propose --from`. See [`DECISIONS.md` §"Lead reconciliation (D2)"](./DECISIONS.md#lead-reconciliation-d2).
- `crates/workflow/src/slice/build/` — target build envelope kernel. `wire.rs` holds the closed-shape `BuildRequest` / `BuildReport` DTOs (round-tripping `schemas/target/build-{request,report}.schema.json`), `BuildOutput` (`{ platform: Platform, path }` — the optional per-platform build outputs declared in `BuildReport.outputs[]`), plus the `enforce_report_no_blocking_on_success` and `enforce_report_outputs_exist` gates; `assemble.rs` assembles a request from the bound target adapter's declared `inputs[]` against the slice tree (raising `target-build-input-missing`). The `specify slice build <slice> [--phase prepare|finalize]` handler ([`src/runtime/commands/slice/build.rs`](./src/runtime/commands/slice/build.rs)) owns request assembly, report validation, the `target-build-*` aborts (including `target-build-output-missing` for absent/empty output paths), the `slice.build.*` events, and the `built` transition gate.
- `crates/workflow/src/journal.rs` — newline-delimited JSON journal event log at `<project_dir>/.specify/journal.jsonl`; closed `Event` / `EventKind` taxonomy with kebab-case wire ids and `snake_case` Rust variants joined by `#[serde(rename = "…")]` (including the single `PlanReconcileCompleted` variant — the former `PlanReconcileAgent` + `PlanReconcileCompleted` pair was folded into one indivisible event and the `ReconcileScope` payload struct removed — plus the RFC-30 bootstrap events `cli.upgraded` / `plugins.refreshed` / `migration.applied` / `migration.skipped`).
- `crates/workflow/src/{migrate,upgrade,plugins}.rs` — the RFC-30 bootstrap lifecycle (handlers in [`src/runtime/commands/{migrate,upgrade,plugins}.rs`](./src/runtime/commands)). `migrate.rs` owns the `#[non_exhaustive]` `MigrationKind` (`V1ToV2`), `MigrationKind::resolve(from, to)`, the `Migrator` trait, `MigrationPlan` / `MigrationReport` / `MigrationAction`, the `apply_staged` atomic harness, and `migrator_for(kind)`; `upgrade.rs` owns `InstallChannel::detect()` and the channel-native upgrade plan; `plugins.rs` owns Cursor plugin-cache discovery and the `doctor` / `refresh` reports. All four bootstrap verbs (`migrate`, `upgrade`, `plugins {doctor,refresh}`, `init --upgrade`) resolve config through the `ProjectConfig::load_for_migration` carve-out, never `load`. See [`DECISIONS.md` §"Bootstrap, upgrade, and migration lifecycle (RFC-30)"](./DECISIONS.md#bootstrap-upgrade-and-migration-lifecycle-rfc-30).
- `crates/schema/src/` — embedded JSON Schema constants (`ADAPTER_JSON_SCHEMA`, `SOURCE_JSON_SCHEMA`, `TARGET_JSON_SCHEMA`, `TOOL_JSON_SCHEMA`, `TOOL_SIDECAR_JSON_SCHEMA`, `PLAN_JSON_SCHEMA`, `EVIDENCE_JSON_SCHEMA`, `LEAD_JSON_SCHEMA`, `PROPOSAL_JSON_SCHEMA`, `SLICE_MODEL_JSON_SCHEMA`, `PROVENANCE_JSON_SCHEMA`, `TOPOLOGY_LOCK_JSON_SCHEMA`, `BUILD_REQUEST_JSON_SCHEMA`, `BUILD_REPORT_JSON_SCHEMA`, `COMPONENTS_JSON_SCHEMA`, `RULE_JSON_SCHEMA`, `RESOLVED_RULES_JSON_SCHEMA`, `DIAGNOSTIC_JSON_SCHEMA`, `DIAGNOSTIC_REPORT_JSON_SCHEMA`, `WORKSPACE_MODEL_JSON_SCHEMA`, `SKILL_JSON_SCHEMA`, `SCENARIO_JSON_SCHEMA`, `MARKETPLACE_JSON_SCHEMA`) and the shared `jsonschema::Validator` plumbing (`compile_schema`, `validate_value`, `validate_serialisable`, `read_yaml_as_json`). Workflow, standards, and tool layers all consume schemas through this crate; nobody else embeds `include_str!`'d schema JSON. The `crates/schema/tests/schemas.rs` parity test asserts each embedded constant byte-matches its on-disk `schemas/` source.
- `crates/diagnostics/src/` — the neutral `Diagnostic` substrate: the `Diagnostic` / `DiagnosticReport` / `DiagnosticSummary` types with the orthogonal `source` (`deterministic | model-assisted | hybrid | human | tool`) and `kind` (`violation | review`) axes, the fingerprint algorithm, `validate_diagnostic`, the four renderers (`json/pretty/github/compact`), and the `blocking` predicate. Import it directly from `specify-diagnostics`; `specify-standards` no longer re-exports the currency.
- `crates/standards/src/rules/` — rules parser and resolver pipeline (`parse.rs`, `resolve.rs`, `resolve/{filter,sort}.rs`). Kept out of `specify-workflow` by the standards-layer split.
- `crates/standards/src/framework/` — framework `Check` substrate (`check/`, `context.rs`, `helpers.rs`, `schema.rs`, `error.rs`). **No framework CORE rule runs here:** every `CORE-*` framework check resolves through the generic lint dispatcher — either a declarative hint (Road A, `lint/eval/*`) or a name-resolved WASI tool (Road B, `wasi-tools/<name>/`). The `kind: authoring-predicate` bridge is fully removed. The only surviving `Check` impls are the repo-local Rust-quality predicates (`RustTestNaming` / `RustSourceQuality`) run through `check::run_rust_quality` for this repo's own `cargo test --test rust_quality` gate; `check/brief.rs` keeps the pure parent/phase brief path-classifiers the indexer reuses. `builder.rs` holds an empty `CORE_ID_TABLE`, the severity table, and `framework_finding()` builders. Posture: [DIAGNOSTICS.md §A16](./DIAGNOSTICS.md), [DECISIONS.md §"Framework lint engine: generic dispatcher (Road A / Road B)"](./DECISIONS.md#framework-lint-engine-generic-dispatcher-road-a--road-b). Contributor model (framework repo): [docs/contributing/checks.md](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md).
- `crates/standards/src/lint/` — `specify lint` and `specify lint framework` surface: `WorkspaceModel` DTOs (`model.rs`), the dual-profile indexer (`index/`), the generic per-kind hint interpreter (`eval/*`), and the shared lint runner. The engine is a rule-agnostic dispatcher: the **Road A** evaluators (`schema`, `reference-resolves`, `cardinality`, `set-coverage`, `set-eq`, `constant-eq`, `content-digest-eq`, `unique`, `fenced-block`, `regex`, `path-pattern`) interpret a declarative hint over `WorkspaceModel` facts, and **Road B** (`eval/tool.rs`) resolves a `kind: tool` hint by name and folds the named WASI tool's `DiagnosticReport`. No eval arm embeds rule policy — each reads its caps/sets/maps from the rule's `config:` (forwarded to Road B tools as a second positional arg); the `lint_no_embedded_policy` guard test enforces this. The renderers it returns are the neutral formatters from `specify-diagnostics`. The framework-profile extractors (`index/{skill,adapter,marketplace,agent_teams,brief}.rs`) sit beside the product pass and run when `lint::index::build(project_dir, ScanProfile::Framework, &[], &[])` is invoked; the §F1 walk driver lives in `index/framework.rs` and follows symlinks (recording both endpoints) while the product profile records-without-traverse. `specify lint framework` is the only caller of the framework profile today.
- `crates/agents/src/` — init-time `AGENTS.md` context-fence generation, extracted from the binary so its pure logic carries unit tests in a workspace crate. Public modules: `detect` (shallow root-marker detection), `render` (deterministic Markdown body + `Input` struct), `fences` (byte-preserving `parse_document` / `plan_agents_write` write planner), `fingerprint` (`InputCollector` + canonical aggregate digest), `lock` (`context.lock` sidecar). All `Ctx`-free; the binary's `src/runtime/commands/agents/{assemble,generate}.rs` adapt a `Ctx` into a `render::Input` and drive these modules. Carries a module-scoped `missing_docs` / `pedantic` / `nursery` allow that preserves the pre-extraction (binary-internal) lint posture.

WASI tools live in the sibling workspace at `wasi-tools/` and are carved out of the host workspace's discipline (deps: `serde` / `serde-saphyr` / `jsonschema` / `regex` only — never the host diagnostics crate; each embeds its own schema copies as mechanism). Two families share the pattern: the **adapter validators** (`wasi-tools/contract`, `wasi-tools/vectis`), consumed through `specify tool run <name>`, and the **nine framework checkers** (`scenarios`, `skill`, `skill-body`, `agent-teams`, `adapter`, `links-registry`, `marketplace`, `prose`, `rules`) that back the Road B `kind: tool` framework rules. Each framework checker ships a prebuilt `dist/<name>-<ver>.wasm` embedded into the binary via `FrameworkToolRunner` ([`src/runtime/commands/lint/framework_tools.rs`](./src/runtime/commands/lint/framework_tools.rs)) — name-resolved with `sha256: None` (digest pinning deferred until the source moves to its colocated home) — and is rebuilt by `cargo make <name>-wasm`. Run `cargo clippy -p <tool> -- -D warnings` inside `wasi-tools/` for any tool change; the host `cargo make lint` does not cover that workspace. All carve-outs are self-contained — plugin-specific validation, scaffold, and rendering logic lives inside the carve-out and the host CLI consumes it only through the tool runner. The vectis tool exposes a `verify` subcommand (`wasi-tools/vectis/src/verify.rs`) with two modes: `--mode detect` (plan-time, returns the set of declared-but-absent platforms as JSON) and `--mode verify` (build/lint-time, emits `diagnostic.schema.json` findings and exits non-zero on any miss for a supported platform). Authority is `project.yaml.platforms`; `web`/`desktop` emit a `platform-not-yet-supported` info finding and are treated as present.

## Exit codes

Part of the CLI wire contract. `Exit::from(&Error)` in [`src/runtime/output.rs`](./src/runtime/output.rs) is the single source of truth.

## Repository map

```text
src/runtime/          specify dispatch (the single CLI; lint project + lint framework)
crates/workflow/        workflow domain logic
crates/standards/src/framework/  framework Check substrate (repo-local rust-quality predicates; no CORE rule producer)
```

| Code | Name | When |
|---|---|---|
| 0 | `EXIT_SUCCESS` | Command succeeded. |
| 1 | `EXIT_GENERIC_FAILURE` | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, …). |
| 2 | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, `Error::Argument`, or an undeclared/over-permissioned tool request. |
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::CliTooOld` — `project.yaml.specify_version` is newer than the binary. |
| 4 | `EXIT_MIGRATION_REQUIRED` | `Error::ProjectNeedsMigration` — `project.yaml.specify_version` major is older than the binary; run `specify migrate`. |

See [DECISIONS.md §"Exit codes"](./DECISIONS.md#exit-codes) for the long-form rationale (including `Exit::Code(u8)`'s WASI passthrough role).

## Documentation map

| Topic | Document |
|---|---|
| Cross-cutting code-quality rules (naming, error variants, traits-for-testability, archaeology) | [`docs/standards/style.md`](./docs/standards/style.md) |
| Lints, comments, brevity, DTOs, YAML/atomic writes, module layout (`<module>.rs` + `<module>/`, no `mod.rs` outside `tests/`) | [`docs/standards/coding-standards.md`](./docs/standards/coding-standards.md) |
| `Ctx`, `Out`/`Render`/`emit`, exit-code mapping, dispatcher contract | [`docs/standards/handler-shape.md`](./docs/standards/handler-shape.md) |
| Workspace layout, WASI carve-outs, `Layout<'a>`, time injection, `ureq` hardening, atomic-write rationale, workflow domain modules, supply chain | [`docs/standards/architecture.md`](./docs/standards/architecture.md) |
| `cargo nextest`, integration-first policy, golden files, `REGENERATE_GOLDENS` | [`docs/standards/testing.md`](./docs/standards/testing.md) |
| Standing architectural decisions (error layering, exit codes, atomic writes, YAML library, wire compatibility, workflow type renames, plan lifecycle, adapter loader, journal events) | [`DECISIONS.md`](./DECISIONS.md) |
| Engineering standards layer (`specify-standards` / `specify-schema`, `WorkspaceModel`, deterministic hints, `specify lint`) | [`DECISIONS.md` §"Standards layer split into `specify-standards` and `specify-schema`](./DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema) |

## Rust quality {#rust-quality}

Read [style.md](./docs/standards/style.md), [coding-standards.md](./docs/standards/coding-standards.md), and [testing.md § Test naming](./docs/standards/testing.md#test-naming) before adding types, suppressions, or tests. Run `cargo make ci` (not bare `cargo test` — CI uses `RUSTFLAGS=-Dwarnings`).

**Naming:** The module path is context — `registry::show`, not `show_registry`. Test function names are short identifiers; put the narrative in the test body ([testing.md](./docs/standards/testing.md#test-naming)).

**Lint suppressions:** Refactor first. Use `#[expect(lint, reason = "…")]` at the smallest scope. `#![allow]` only at module root when the lint applies to every item below and the reason is contract-locked. `#[allow]` without `reason` fails CI.

**Rust-quality CI:** `cargo test --test rust_quality` runs `RustTestNaming` and `RustSourceQuality` over this repo (long test fn names, archaeology in `//!`/`///`, bare `#[allow]`). Findings are burn-down tracked in `docs/quality-debt.md`.

| Do not | Do instead | See |
| --- | --- | --- |
| `#[allow]` / `#[expect]` before trying a split or extract | Extract helper or submodule; suppress only if contract-locked | [coding-standards § Lint suppression](./docs/standards/coding-standards.md#lint-suppression-posture) |
| `trait Foo` + sole `RealFoo` for tests | `CmdRunner`, `AtomicYaml`, or filesystem/tempdir | [style.md § No traits for testability](./docs/standards/style.md#no-traits-for-testability-alone) |
| `*RenderInput` wrapper for `Render` | `Render` on domain type or `ctx.emit_with` closure | [style.md § One body per command](./docs/standards/style.md#one-body-per-command-no-wrapper-newtype) |
| `match ctx.format { Json, Text }` in handlers | `ctx.write` / `output::report` | [handler-shape.md](./docs/standards/handler-shape.md) |
| RFC/Phase/migration history in `//!` / `///` | ≤ 3 lines “what today”; history in [DECISIONS.md](./DECISIONS.md) | [style.md § No archaeology](./docs/standards/style.md#no-archaeology-in-code) |
| Sentence-length test fn names | Short name + `mod` grouping | [testing.md § Test naming](./docs/standards/testing.md#test-naming) |
| Nested `struct Body` inside `fn` | Top-level `*Body` + `From` impl | [coding-standards § DTOs](./docs/standards/coding-standards.md#dtos) |
| New `Error::Diag` for one-off shapes | Typed variant after ≥3 identical call sites | [style.md § Error variants](./docs/standards/style.md#error-variants-budgeted-by-recovery-not-source) |

External references:

- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [`docs/standards/workflow.md`](./docs/standards/workflow.md) — the in-force workflow contract this binary implements. Defines the `source` / `target` / `plugin` / `axis` vocabulary, the kebab-case wire format, the `Source` / `Lead` / `Evidence` / `Slice` implementation types, writer ownership, and the CLI surface. Stable `§`-anchors that source comments and skill briefs cite by name.
- [`docs/release.md`](./docs/release.md) — tagging and crates.io publish pipeline.
- [`schemas/`](./schemas/) — JSON Schema files distributed with the binary (`adapter.schema.json`, `source.schema.json`, `target.schema.json`, `evidence.schema.json`, `discovery/lead.schema.json`, `plan/plan.schema.json`, `target/build-request.schema.json`, and `target/build-report.schema.json`); the workflow contract pins each shape.

## Quick toolchain

All driven by `cargo make` (see [`Makefile.toml`](./Makefile.toml)). Run the full local CI suite before committing; do not rely on narrower substitutes such as `cargo test` or `cargo clippy`.

```bash
cargo make ci             # lint + test + test-docs + doc + vet + outdated + deny + fmt
cargo make check          # fmt + lint + test + test-docs (the pre-commit subset)
cargo make test           # cargo nextest run --all --all-features --no-tests=pass under -Dwarnings
cargo make lint           # cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo make fmt            # nightly cargo fmt --all
cargo make audit          # cargo-audit; cargo make deny / outdated / deps / vet for the rest
cargo make contract-wasm  # build wasi-tools/contract — required before tests/contract_tool.rs
```

Less frequent recipes:

```bash
scripts/regen-wasm-fixtures.sh   # regenerate the checked-in WASI fixtures under tests/fixtures/tools-test-*/wasm/
scripts/build-vectis-local.sh    # build wasi-tools/vectis with sha256 sidecars for local smoke tests
```

## When working in this repo

1. Read [`DECISIONS.md`](./DECISIONS.md) before changing error layering, exit codes, atomic writes, the YAML library, the JSON envelope shape, the workflow type names (`Target*` / `Plugin` / `SliceSourceBinding` / `Divergence`), the plan lifecycle (`pending | approved`), the journal event taxonomy, the per-axis cache layout, or adding a new workspace crate.
2. For any Rust change, consult [`docs/standards/`](./docs/standards/) — at minimum the doc that matches the area you are editing, plus [`style.md`](./docs/standards/style.md) for cross-cutting rules.
3. Run `cargo make ci` before committing. If it cannot run, say exactly why and which checks were run instead.
4. When you remove a symbol, `rg <SymbolName> -- AGENTS.md DECISIONS.md docs/` and update every hit in the same PR.
5. If you touch `Slice.target`, `SliceSourceBinding`, `Divergence`, `crates/model/src/spec/provenance.rs`, `crates/workflow/src/adapter/`, `crates/workflow/src/change/plan/core/propose.rs`, `crates/workflow/src/journal.rs`, `crates/schema/src/`, `crates/standards/src/rules/`, `crates/standards/src/lint/`, the `$CAPABILITY_DIR` env var, or the `adapter--<axis>--<slug>` tool cache scope: `rg <symbol>` across both this repo *and* the parent [`augentic/specify`](https://github.com/augentic/specify) plugin repo, and update every hit in the same PR (workflow §"Note to the implementing agent" applies — the workflow contract spans both repos).
6. A fresh contributor should be able to reach any rule from this spine in three hops or fewer. If you find yourself adding prose here that isn't navigational, it belongs in one of the standards docs.
7. For Rust changes, skim [Rust quality](#rust-quality) before adding types, suppressions, or tests; if you add `#[expect]`, state in the PR why a refactor was infeasible.
