# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specify` binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives those workflows compose.

## Crate graph

The workspace is leaf → root. `specify-error` is the dependency leaf and depends on no other workspace crate.

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-tool                     # depends on specify-error (WASI tool runner; wasmtime, gated)
specify-domain                   # depends on specify-{error,tool} (every other domain module)
specify (root crate)             # wires every workspace crate above into the CLI binary
```

Modules of note inside `specify-domain` (RFC-25 reshapes from Wave 0):

- `crates/domain/src/plugin/` — axis-aware loader (`Axis::Source` / `Axis::Target`). Replaces the pre-RFC-25 `crate::adapter` shared-shape loader; the remaining `crate::adapter` surface keeps narrower concerns (`Brief`, `ChangeBrief`, `CodexProvenance`, `PipelineView`, `CacheMeta`).
- `crates/domain/src/schema.rs` — JSON Schemas (`plan.yaml`, per-source `Evidence`, plugin/source/target manifests) embedded via `include_str!` and validated through `jsonschema::Validator`.
- `crates/domain/src/spec/provenance.rs` — `spec.md` requirement-block parser (`ID:` / `Sources:` / `Status:` lines, closed `RequirementStatus` enum, inline `[…]` tag coherence).
- `crates/domain/src/journal.rs` — RFC-19 newline-delimited JSON event log at `<project_dir>/.specify/journal.jsonl`; closed `Event` / `EventKind` taxonomy with kebab-case wire ids and `snake_case` Rust variants joined by `#[serde(rename = "…")]`.

WASI tools live in the sibling workspace at `wasi-tools/` (`wasi-tools/contract`, `wasi-tools/vectis`) and are carved out of the host workspace's discipline. Both carve-outs are self-contained — plugin-specific validation, scaffold, and rendering logic lives inside the carve-out and the host CLI consumes it only through `specify tool run <name>`.

## Exit codes

Part of the CLI wire contract. `Exit::from(&Error)` in [`src/output.rs`](./src/output.rs) is the single source of truth.

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
| Workspace layout, WASI carve-outs, `Layout<'a>`, time injection, `ureq` hardening, atomic-write rationale, RFC-25 domain modules, supply chain | [`docs/standards/architecture.md`](./docs/standards/architecture.md) |
| `cargo nextest`, integration-first policy, golden files, `REGENERATE_GOLDENS` | [`docs/standards/testing.md`](./docs/standards/testing.md) |
| Standing architectural decisions (error layering, exit codes, atomic writes, YAML library, wire compatibility, RFC-25 type renames, plan lifecycle, plugin loader, journal events) | [`DECISIONS.md`](./DECISIONS.md) |

External references:

- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [Parent repo `rfcs/rfc-25-workflow.md`](https://github.com/augentic/specify/blob/main/rfcs/rfc-25-workflow.md) — the active workflow contract. Ships as Specify 2.0 and supersedes the archived RFC-20 (survey) and RFC-23 (change-lifecycle). Defines the `source` / `target` / `plugin` / `axis` vocabulary, the kebab-case wire format, the `Source` / `Candidate` / `Evidence` / `Slice` implementation types, writer ownership, the CLI surface this binary commits to, and the plan-lock contract.
- [Parent repo `rfcs/`](https://github.com/augentic/specify/tree/main/rfcs) — full active + archived RFC index.
- [`docs/release.md`](./docs/release.md) — tagging and crates.io publish pipeline.
- [`schemas/`](./schemas/) — JSON Schema files distributed with the binary (including the RFC-25 `plugin.schema.json`, `source.schema.json`, `target.schema.json`, `evidence.schema.json`, `discovery/candidate.schema.json`, and the refined `plan/plan.schema.json`).

## Quick toolchain

All driven by `cargo make` (see [`Makefile.toml`](./Makefile.toml)). Run the full local CI suite before committing; do not rely on narrower substitutes such as `cargo test` or `cargo clippy`.

```bash
cargo make ci             # lint + file-size + test + test-docs + doc + vet + outdated + deny + fmt
cargo make check          # fmt + lint + test + test-docs (the pre-commit subset)
cargo make test           # cargo nextest run --all --all-features --no-tests=pass under -Dwarnings
cargo make lint           # cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo make fmt            # nightly cargo fmt --all
cargo make audit          # cargo-audit; cargo make deny / outdated / deps / vet for the rest
cargo make xtask gen-man  # roff man pages into target/man/
cargo make contract-wasm  # build wasi-tools/contract — required before tests/contract_tool.rs
```

Less frequent recipes:

```bash
scripts/regen-wasm-fixtures.sh   # regenerate the checked-in WASI fixtures under tests/fixtures/tools-test-*/wasm/
scripts/build-vectis-local.sh    # build wasi-tools/vectis with sha256 sidecars for local smoke tests
```

## When working in this repo

1. Read [`DECISIONS.md`](./DECISIONS.md) before changing error layering, exit codes, atomic writes, the YAML library, the JSON envelope shape, the RFC-25 type names (`Target*` / `Plugin` / `SliceSourceBinding` / `Divergence`), the plan lifecycle (`pending | reviewed`), the journal event taxonomy, the per-axis cache layout, or adding a new workspace crate.
2. For any Rust change, consult [`docs/standards/`](./docs/standards/) — at minimum the doc that matches the area you are editing, plus [`style.md`](./docs/standards/style.md) for cross-cutting rules.
3. Run `cargo make ci` before committing. If it cannot run, say exactly why and which checks were run instead.
4. When you remove a symbol, `rg <SymbolName> -- AGENTS.md DECISIONS.md docs/` and update every hit in the same PR.
5. If you touch `Slice.target`, `SliceSourceBinding`, `Plan::resolve_sources`, `Divergence`, `crates/domain/src/spec/provenance.rs`, `crates/domain/src/plugin/`, `crates/domain/src/journal.rs`, `crates/domain/src/schema.rs`, the `$CAPABILITY_DIR` env var, or the `plugin--<axis>--<slug>` tool cache scope: `rg <symbol>` across both this repo *and* the parent [`augentic/specify`](https://github.com/augentic/specify) plugin repo, and update every hit in the same PR (RFC-25 §"Note to the implementing agent" applies — the workflow contract spans both repos).
6. A fresh contributor should be able to reach any rule from this spine in three hops or fewer. If you find yourself adding prose here that isn't navigational, it belongs in one of the standards docs.
