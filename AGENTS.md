# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specrun` runtime binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives those workflows compose.

## Crate graph

The workspace is leaf → root. `specify-error` is the dependency leaf and depends on no other workspace crate.

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-schema                   # depends on specify-error (embedded JSON Schemas + jsonschema plumbing)
specify-tool                     # depends on specify-error (WASI tool runner; wasmtime, gated)
specify-lints                    # standards layer — depends on specify-{error,schema,tool}; NOT on specify-domain
specify-domain                   # workflow layer — depends on specify-{error,schema,tool}; NOT on specify-lints
specify (root crate)             # wires every workspace crate above into the CLI binary
```

`specify-lints` and `specify-domain` are siblings: neither imports the other. The standards-layer-vs-workflow-layer split is a type-system invariant by the dependency-direction invariant in [DECISIONS.md §"Standards layer split into `specify-lints` and `specify-schema"](./DECISIONS.md#standards-layer-split-into-specify-lints-and-specify-schema) (lint carries no lifecycle authority). Both depend on `specify-schema` so the embedded JSON Schemas live in one place.

Modules of note across the workspace (workflow + standards layers):

- `crates/domain/src/adapter/` — axis-split adapter loader. `SourceAdapter::resolve(name, project_dir)` and `TargetAdapter::resolve(name, project_dir)` are the per-axis entry points and the only manifest loaders after the F9 collapse + workflow §"Operations typed at parse boundary" split (the legacy axis-generic `Adapter::resolve(axis, …)` shape and `PipelineView` were retired). The closed `SourceOperation` / `TargetOperation` enums in `adapter/operation.rs` are the typed `briefs.keys()` carried by each manifest struct.
- `crates/domain/src/spec/provenance.rs` — `spec.md` requirement-block parser (`ID:` / `Sources:` / `Status:` lines, closed `RequirementStatus` enum, inline `[…]` tag coherence).
- `crates/domain/src/journal.rs` — newline-delimited JSON journal event log at `<project_dir>/.specify/journal.jsonl`; closed `Event` / `EventKind` taxonomy with kebab-case wire ids and `snake_case` Rust variants joined by `#[serde(rename = "…")]`.
- `crates/schema/src/` — embedded JSON Schema constants (`PLAN_JSON_SCHEMA`, `EVIDENCE_JSON_SCHEMA`, `FUSION_JSON_SCHEMA`, `COMPONENTS_JSON_SCHEMA`, `RULE_JSON_SCHEMA`, `RESOLVED_RULES_JSON_SCHEMA`, `LINT_FINDING_JSON_SCHEMA`, `LINT_RESULT_JSON_SCHEMA`, `WORKSPACE_MODEL_JSON_SCHEMA`) and the shared `jsonschema::Validator` plumbing (`compile_schema`, `validate_value`, `validate_serialisable`, `read_yaml_as_json`). Workflow and standards layers both consume schemas through this crate; nobody else embeds `include_str!`'d schema JSON.
- `crates/specify-lints/src/rules/` — rules parser, resolver pipeline, fingerprint algorithm, and finding validator (`parse.rs`, `resolve.rs`, `resolve/{filter,sort}.rs`, `fingerprint.rs`, `finding.rs`). Kept out of `specify-domain` by the standards-layer split.
- `crates/specify-lints/src/lint/` — `specrun lint` surface: `WorkspaceModel` DTOs (`model.rs`), the consumer indexer (`index/`), the deterministic hint interpreter for the closed Phase 2 kinds (`eval/{path_pattern,regex,schema,tool}.rs`), and the four diagnostic formatters (`diagnostics/{json,pretty,github,compact}.rs`) that back `specrun lint`.

WASI tools live in the sibling workspace at `wasi-tools/` (`wasi-tools/contract`, `wasi-tools/vectis`) and are carved out of the host workspace's discipline. Both carve-outs are self-contained — plugin-specific validation, scaffold, and rendering logic lives inside the carve-out and the host CLI consumes it only through `specrun tool run <name>`.

## Exit codes

Part of the CLI wire contract. `Exit::from(&Error)` in [`src/runtime/output.rs`](./src/runtime/output.rs) is the single source of truth.

## Repository map

```text
src/runtime/          specrun dispatch (workflow CLI)
src/authoring/        specdev dispatch (framework checks CLI)
crates/domain/        workflow domain logic
crates/authoring/     check predicates (library; not the specdev binary tree)
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
| Engineering standards layer (`specify-lints` / `specify-schema`, `WorkspaceModel`, deterministic hints, `specrun lint`) | [`DECISIONS.md` §"Standards layer split into `specify-lints` and `specify-schema`](./DECISIONS.md#standards-layer-split-into-specify-lints-and-specify-schema) |

External references:

- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [`docs/standards/workflow.md`](./docs/standards/workflow.md) — the in-force workflow contract this binary implements. Defines the `source` / `target` / `plugin` / `axis` vocabulary, the kebab-case wire format, the `Source` / `Candidate` / `Evidence` / `Slice` implementation types, writer ownership, and the CLI surface. Stable `§`-anchors that source comments and skill briefs cite by name.
- [`docs/release.md`](./docs/release.md) — tagging and crates.io publish pipeline.
- [`schemas/`](./schemas/) — JSON Schema files distributed with the binary (`adapter.schema.json`, `source.schema.json`, `target.schema.json`, `evidence.schema.json`, `discovery/candidate.schema.json`, and `plan/plan.schema.json`); the workflow contract pins each shape.

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
5. If you touch `Slice.target`, `SliceSourceBinding`, `Divergence`, `crates/domain/src/spec/provenance.rs`, `crates/domain/src/adapter/`, `crates/domain/src/journal.rs`, `crates/schema/src/`, `crates/specify-lints/src/rules/`, `crates/specify-lints/src/lint/`, the `$CAPABILITY_DIR` env var, or the `adapter--<axis>--<slug>` tool cache scope: `rg <symbol>` across both this repo *and* the parent [`augentic/specify`](https://github.com/augentic/specify) plugin repo, and update every hit in the same PR (workflow §"Note to the implementing agent" applies — the workflow contract spans both repos).
6. A fresh contributor should be able to reach any rule from this spine in three hops or fewer. If you find yourself adding prose here that isn't navigational, it belongs in one of the standards docs.
