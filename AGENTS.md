# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specrun` runtime binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives those workflows compose.

## Crate graph

The workspace is leaf → root. `specify-error` is the dependency leaf and depends on no other workspace crate.

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-schema                   # depends on specify-error (embedded JSON Schemas + jsonschema plumbing)
specify-diagnostics              # leaf — depends on specify-{error,schema} (Diagnostic substrate: report, fingerprint, validator, renderers, blocking)
specify-model                    # depends on specify-{error,diagnostics} (artifact types + parsers: spec, task, evidence, discovery; shared atomic writer)
specify-tool                     # depends on specify-{error,diagnostics} (WASI tool runner; wasmtime, gated)
specify-validate                 # depends on specify-{model,error,diagnostics} — artifact rule registry; NOT on specify-workflow or anything named lint
specify-standards                # standards layer — depends on specify-{error,schema,tool,diagnostics}; NOT on specify-workflow
specify-workflow                 # workflow layer — depends on specify-{error,schema,tool,model,diagnostics}; NOT on specify-standards / specify-validate
specify (root crate)             # wires every workspace crate above into the CLI binary
```

`specify-standards` and `specify-workflow` are siblings: neither imports the other. The standards-layer-vs-workflow-layer split is a type-system invariant by the dependency-direction invariant in [DECISIONS.md §"Standards layer split into `specify-standards` and `specify-schema"](./DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema) (lint carries no lifecycle authority). `specify-validate` mirrors that invariant for artifact validation: it depends on `specify-model` only, never on `specify-workflow`, so a rule cannot transition a slice or stamp a plan. `specify-model` is the lifecycle-free leaf holding the artifact types and parsers both higher layers read. Both standards and workflow depend on `specify-schema` so the embedded JSON Schemas live in one place. The neutral `Diagnostic` / `DiagnosticReport` substrate lives in the `specify-diagnostics` leaf (depends only on `specify-{error,schema}`), so every check producer — validate and lint alike — emits the same finding currency without `specify-validate` (or any non-lint producer) depending on anything named `lint`. See [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](./DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate).

Modules of note across the workspace (workflow + standards layers):

- `crates/workflow/src/adapter/` — axis-split adapter loader. `SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points and the only manifest loaders after the F9 collapse + workflow §"Operations typed at parse boundary" split (the legacy axis-generic `Adapter::resolve(axis, …)` shape and `PipelineView` were retired). The closed `SourceOperation` / `TargetOperation` enums in `adapter/operation.rs` are the typed `briefs.keys()` carried by each manifest struct.
- `crates/model/src/spec/provenance.rs` — `spec.md` requirement-block parser (`ID:` / `Sources:` / `Status:` lines, closed `RequirementStatus` enum, inline `[…]` tag coherence).
- `crates/workflow/src/journal.rs` — newline-delimited JSON journal event log at `<project_dir>/.specify/journal.jsonl`; closed `Event` / `EventKind` taxonomy with kebab-case wire ids and `snake_case` Rust variants joined by `#[serde(rename = "…")]`.
- `crates/schema/src/` — embedded JSON Schema constants (`PLAN_JSON_SCHEMA`, `EVIDENCE_JSON_SCHEMA`, `PROVENANCE_JSON_SCHEMA`, `COMPONENTS_JSON_SCHEMA`, `RULE_JSON_SCHEMA`, `RESOLVED_RULES_JSON_SCHEMA`, `DIAGNOSTIC_JSON_SCHEMA`, `DIAGNOSTIC_REPORT_JSON_SCHEMA`, `WORKSPACE_MODEL_JSON_SCHEMA`, `SKILL_JSON_SCHEMA`, `SCENARIO_JSON_SCHEMA`, `MARKETPLACE_JSON_SCHEMA`) and the shared `jsonschema::Validator` plumbing (`compile_schema`, `validate_value`, `validate_serialisable`, `read_yaml_as_json`). Workflow and standards layers both consume schemas through this crate; nobody else embeds `include_str!`'d schema JSON.
- `crates/diagnostics/src/` — the neutral `Diagnostic` substrate: the `Diagnostic` / `DiagnosticReport` / `DiagnosticSummary` types with the orthogonal `source` (`deterministic | model-assisted | hybrid | human | tool`) and `kind` (`violation | review`) axes, the fingerprint algorithm, `validate_diagnostic`, the four renderers (`json/pretty/github/compact`), and the `blocking` predicate. Import it directly from `specify-diagnostics`; `specify-standards` no longer re-exports the currency.
- `crates/standards/src/rules/` — rules parser and resolver pipeline (`parse.rs`, `resolve.rs`, `resolve/{filter,sort}.rs`). Kept out of `specify-workflow` by the standards-layer split.
- `crates/standards/src/framework/` — the dissolved `specify-authoring` crate: the imperative `Check` predicates behind `specdev lint` (`check/`, `context.rs`, `helpers.rs`, `schema.rs`, `error.rs`). `builder.rs` holds the `CORE_ID_TABLE`, severity table, and `framework_finding()` / `loc()` builders; every predicate emits a canonical `Diagnostic` directly, and `framework::check::run` finalizes (rebase → fingerprint → `FIND-NNNN` ids). The declarative burn-down deletes these as each predicate migrates to a `CORE-NNN` rule file.
- `crates/standards/src/lint/` — `specrun lint` and `specdev lint` surface: `WorkspaceModel` DTOs (`model.rs`), the dual-profile indexer (`index/`), the deterministic hint interpreter for the closed Phase 2 kinds (`eval/{path_pattern,regex,schema,tool}.rs`), and the shared lint runner. The renderers it returns are the neutral formatters from `specify-diagnostics`. The framework-profile extractors (`index/{skill,adapter,marketplace,agent_teams,brief}.rs`) sit beside the consumer pass and run when `lint::index::build(project_dir, ScanProfile::Framework, &[], &[])` is invoked; the §F1 walk driver lives in `index/framework.rs` and follows symlinks (recording both endpoints) while the consumer profile records-without-traverse. `specdev lint` is the only caller of the framework profile today.

WASI tools live in the sibling workspace at `wasi-tools/` (`wasi-tools/contract`, `wasi-tools/vectis`) and are carved out of the host workspace's discipline. Both carve-outs are self-contained — plugin-specific validation, scaffold, and rendering logic lives inside the carve-out and the host CLI consumes it only through `specrun tool run <name>`.

## Exit codes

Part of the CLI wire contract. `Exit::from(&Error)` in [`src/runtime/output.rs`](./src/runtime/output.rs) is the single source of truth.

## Repository map

```text
src/runtime/          specrun dispatch (workflow CLI)
src/authoring/        specdev dispatch (framework checks CLI)
crates/workflow/        workflow domain logic
crates/standards/src/framework/  imperative framework check predicates (behind specdev lint)
```

| Code | Name | When |
|---|---|---|
| 0 | `EXIT_SUCCESS` | Command succeeded. |
| 1 | `EXIT_GENERIC_FAILURE` | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, …). |
| 2 | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, `Error::Argument`, or an undeclared/over-permissioned tool request. |
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::CliTooOld` — `project.yaml.specify_version` is newer than the binary. |

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
| Engineering standards layer (`specify-standards` / `specify-schema`, `WorkspaceModel`, deterministic hints, `specrun lint`) | [`DECISIONS.md` §"Standards layer split into `specify-standards` and `specify-schema`](./DECISIONS.md#standards-layer-split-into-specify-standards-and-specify-schema) |

External references:

- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [`docs/standards/workflow.md`](./docs/standards/workflow.md) — the in-force workflow contract this binary implements. Defines the `source` / `target` / `plugin` / `axis` vocabulary, the kebab-case wire format, the `Source` / `Lead` / `Evidence` / `Slice` implementation types, writer ownership, and the CLI surface. Stable `§`-anchors that source comments and skill briefs cite by name.
- [`docs/release.md`](./docs/release.md) — tagging and crates.io publish pipeline.
- [`schemas/`](./schemas/) — JSON Schema files distributed with the binary (`adapter.schema.json`, `source.schema.json`, `target.schema.json`, `evidence.schema.json`, `discovery/lead.schema.json`, and `plan/plan.schema.json`); the workflow contract pins each shape.

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
5. If you touch `Slice.target`, `SliceSourceBinding`, `Divergence`, `crates/model/src/spec/provenance.rs`, `crates/workflow/src/adapter/`, `crates/workflow/src/journal.rs`, `crates/schema/src/`, `crates/standards/src/rules/`, `crates/standards/src/lint/`, the `$CAPABILITY_DIR` env var, or the `adapter--<axis>--<slug>` tool cache scope: `rg <symbol>` across both this repo *and* the parent [`augentic/specify`](https://github.com/augentic/specify) plugin repo, and update every hit in the same PR (workflow §"Note to the implementing agent" applies — the workflow contract spans both repos).
6. A fresh contributor should be able to reach any rule from this spine in three hops or fewer. If you find yourself adding prose here that isn't navigational, it belongs in one of the standards docs.
