# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specify` binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives those workflows compose.

## Crate graph

The workspace is leaf → root. `specify-error` is the dependency leaf and depends on no other workspace crate.

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-{registry,capability,task,spec,tool}   # depend on specify-error (spec has no workspace deps)
specify-slice                    # depends on specify-{error,capability,registry}
specify-{merge,config,validate}  # depend on specify-error + the leaves they need
specify-{change,init}            # depend on specify-{error,config,registry,...}
specify (root crate)             # wires every workspace crate above into the CLI binary
```

WASI tools live in the sibling workspace at `wasi-tools/` (`wasi-tools/contract`, `wasi-tools/vectis`) and are carved out of the host workspace's discipline.

## Exit codes

Part of the CLI wire contract. `Exit::from(&Error)` in [`src/output.rs`](./src/output.rs) is the single source of truth.

| Code | Name | When |
|---|---|---|
| 0 | `EXIT_SUCCESS` | Command succeeded. |
| 1 | `EXIT_GENERIC_FAILURE` | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, …). |
| 2 | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, `Error::Argument`, or an undeclared/over-permissioned tool request. |
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::CliTooOld` — `project.yaml.specify_version` is newer than the binary. |

`Exit::Code(u8)` is reserved for `specify tool run` WASI passthrough; it is not for ad-hoc subcommand use. See [DECISIONS.md §"Exit codes"](./DECISIONS.md#exit-codes) for the long-form rationale.

## Documentation map

| Topic | Document |
|---|---|
| Cross-cutting code-quality rules (naming, error variants, traits-for-testability, archaeology) | [`docs/standards/style.md`](./docs/standards/style.md) |
| Lints, comments, brevity, DTOs, YAML/atomic writes, module layout (`<module>.rs` + `<module>/`, no `mod.rs` outside `tests/`) | [`docs/standards/coding-standards.md`](./docs/standards/coding-standards.md) |
| `Ctx`, `Out`/`Render`/`emit`, exit-code mapping, dispatcher contract | [`docs/standards/handler-shape.md`](./docs/standards/handler-shape.md) |
| Workspace layout, WASI carve-outs, `Layout<'a>`, time injection, `ureq` hardening, atomic-write rationale, supply chain | [`docs/standards/architecture.md`](./docs/standards/architecture.md) |
| `cargo nextest`, integration-first policy, golden files, `REGENERATE_GOLDENS` | [`docs/standards/testing.md`](./docs/standards/testing.md) |
| Mechanically enforced predicates (`cargo make standards`) | [`docs/standards/predicates.md`](./docs/standards/predicates.md) |
| Standing architectural decisions (error layering, exit codes, atomic writes, YAML library, wire compatibility, new workspace crates) | [`DECISIONS.md`](./DECISIONS.md) |

External references:

- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [Parent repo `rfcs/`](https://github.com/augentic/specify/tree/main/rfcs) — active and archived RFCs.
- [`docs/release.md`](./docs/release.md) — tagging and crates.io publish pipeline.
- [`schemas/`](./schemas/) — JSON Schema files distributed with the binary.

## Quick toolchain

All driven by `cargo make` (see [`Makefile.toml`](./Makefile.toml)). Run the full local CI suite before committing; do not rely on narrower substitutes such as `cargo test` or `cargo clippy`.

```bash
cargo make ci             # lint + standards + test + test-docs + doc + vet + outdated + deny + fmt
cargo make test           # cargo nextest run --all --all-features --no-tests=pass under -Dwarnings
cargo make standards      # cargo run -p xtask -- standards-check
cargo make lint           # cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo make fmt            # nightly cargo fmt --all
cargo make audit          # cargo-audit; cargo make deny / outdated / deps / vet for the rest
cargo make xtask gen-man  # roff man pages into target/man/ (also: gen-completions)
cargo make contract-wasm  # build wasi-tools/contract — required before tests/contract_tool.rs
```

Less frequent recipes:

```bash
scripts/regen-wasm-fixtures.sh   # regenerate the checked-in WASI fixtures under tests/fixtures/tools-test-*/wasm/
scripts/build-vectis-local.sh    # build wasi-tools/vectis with sha256 sidecars for local smoke tests
```

## When working in this repo

1. Read [`DECISIONS.md`](./DECISIONS.md) before changing error layering, exit codes, atomic writes, the YAML library, the `ENVELOPE_VERSION` wire shape, or adding a new workspace crate.
2. For any Rust change, consult [`docs/standards/`](./docs/standards/) — at minimum the doc that matches the area you are editing, plus [`style.md`](./docs/standards/style.md) for cross-cutting rules.
3. Run `cargo make ci` before committing. If it cannot run, say exactly why and which checks were run instead.
4. When you remove a symbol, `rg <SymbolName> -- AGENTS.md DECISIONS.md docs/` and update every hit in the same PR.
5. A fresh contributor should be able to reach any rule from this spine in three hops or fewer. If you find yourself adding prose here that isn't navigational, it belongs in one of the standards docs.
