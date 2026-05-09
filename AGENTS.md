# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specify` binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives that those workflows compose.

## Workspace layout

Binary + library crate (`name = "specify"`) at the repo root, with workspace member crates under `crates/`. Dependency direction is leaf → root:

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-capability               # depends on specify-error
specify-spec | specify-task      # depend on specify-capability
specify-slice | specify-merge    # depend on specify-spec
specify-validate                 # depends on specify-spec
specify-change                   # depends on specify-slice + specify-spec
specify-tool                     # WASI tool runner (wasmtime); leaf-ish
specify (root crate)             # wires everything for the CLI binary
crates/contract-validate         # WASI component, builds for wasm32-wasip2
crates/vectis-{validate,scaffold} # WASI components, ditto
```

Every crate uses the shared `[workspace.package]` (`edition = "2024"`, `rust-version = "1.93"`, MIT/Apache-2.0) and the shared `[workspace.lints]` block in the root `Cargo.toml` (clippy `all`/`cargo`/`nursery`/`pedantic` warned, plus a hand-picked `restriction` subset).

Hard dependency rule: `specify-error` is the leaf and depends on no other workspace crate. Adding a workspace dep to `specify-error` re-introduces the cycle [DECISIONS.md "Change H"](./DECISIONS.md) was written to avoid; do not.

## Toolchain

Rust stable per `rust-toolchain.toml` (channel `stable`, components `clippy`, `rust-src`, `rustfmt`). WASM targets pre-installed via `targets = ["aarch64-apple-darwin", "wasm32-wasip2", "x86_64-apple-darwin"]`.

`rustfmt.toml` uses unstable nightly features (`unstable_features = true`, `imports_granularity = "Module"`, `group_imports = "StdExternalCrate"`). Format with nightly:

```bash
cargo +nightly fmt --all
```

`cargo make fmt` does this for you.

## Commands

All driven by `cargo make` (see `Makefile.toml`). The bare `Makefile` is a one-line passthrough.

- `cargo make ci` — `lint test test-docs doc vet outdated deny fmt`. The CI workflow at `.github/workflows/ci.yaml` runs the shared `augentic/.github` ci pipeline.
- `cargo make test` — `cargo nextest run --all --all-features --no-tests=pass` with `RUSTFLAGS=-Dwarnings` and a clean prelude. Use this rather than `cargo test`.
- `cargo make lint` — `cargo clippy --all-features`.
- `cargo make fmt` — nightly `cargo fmt --all`.
- `cargo make audit` / `deny` / `outdated` / `deps` — supply-chain checks (cargo-audit, cargo-deny, cargo-outdated, cargo-udeps). `cargo make vet` regenerates `supply-chain/{audits,exemptions,unpublished}.toml` and runs `cargo vet --locked`.
- `cargo make tools-test-fixtures` — rebuild WASI fixture components used by `tests/tool.rs`.
- `cargo make contract-validator-wasm` / `vectis-validate-wasm` / `vectis-scaffold-wasm` / `vectis-wasi-artifacts` — build the WASI tool components for distribution.

Before committing, run the complete local CI suite with `cargo make ci` and fix any failures or warnings it surfaces. Do not rely on narrower substitutes such as `cargo test` or `cargo clippy`; if `cargo make ci` cannot be run, say exactly why and which checks were run instead.

## Lints

Workspace lints live in `Cargo.toml`. Defaults are aggressive — clippy `all`/`cargo`/`nursery`/`pedantic` are all `warn`, plus a curated set of `restriction` lints (`assertions_on_result_states`, `clone_on_ref_ptr`, `map_err_ignore`, `redundant_type_annotations`, `unused_result_ok`, `if_then_some_else_none`, etc.). Compile under `RUSTFLAGS=-Dwarnings` (`cargo make test` does this), so any new warning fails CI.

When you must silence a lint, use `#[allow(<lint>, reason = "…")]` at the smallest possible scope. `clippy.toml` allows `GitHub`, `OAuth`, `OpenTelemetry`, `WebAssembly`, `YAML` as doc idents — extend it (not the surrounding doc comment) when a new proper noun trips `doc_markdown`.

`taplo.toml` formats `Cargo.toml` files. Dependency arrays under `*-dependencies` and `dependencies` reorder alphabetically; preserve that on edit.

## Error handling and exit codes

`specify-error::Error` is the only error type the CLI surfaces. Every fallible function returns `Result<T, specify_error::Error>` (often via `Result<T, Error>` with a per-crate `use`). New error variants land in `crates/error/src/lib.rs` with a stable kebab-case identifier in the `#[error("…")]` message — those identifiers are part of the public contract that skills and tests grep for.

The four-slot CLI exit-code table is fixed (see [DECISIONS.md §"Change I"](./DECISIONS.md#change-i--cli-exit-codes-and-version-floor-semantics)):

| Code | Name | When |
|---|---|---|
| 0 | `EXIT_SUCCESS` | Command succeeded |
| 1 | `EXIT_GENERIC_FAILURE` | Default `Error` → exit 1 |
| 2 | `EXIT_VALIDATION_FAILED` | `Error::Validation`, undeclared/over-permissioned tool |
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::SpecifyVersionTooOld` |

`CliResult::from(&Error)` in `src/output.rs` is the single source of truth. Every dispatcher in `src/commands/*` routes its terminal error through it. Do not invent new exit codes.

`unwrap()` and `expect()` are reserved for invariants the type system can't express (e.g. "this enum variant covers `Status::ALL`" — see `src/commands/status.rs`). Always include a justification string in `expect`. User-facing errors must surface as `Error::*` variants, not panics.

## YAML, JSON, and atomic writes

YAML (de)serialization goes through `serde-saphyr`, not `serde_yaml_ng` (retired) or `serde_yaml` (deprecated). `serde-saphyr` has no `Value` type; for dynamic YAML access deserialize into `serde_json::Value` (see [DECISIONS.md "YAML library"](./DECISIONS.md)). Errors split into `serde_saphyr::Error` (deser) and `serde_saphyr::ser::Error` (ser), and `specify-error::Error` carries both via `Yaml(#[from] …)` and `YamlSer(#[from] …)`.

Writes that must not be observed mid-update use `tempfile::NamedTempFile::new_in(parent).persist(target)`. `Plan::save` is the canonical example; `ChangeMetadata::save` was migrated to the same pattern in L2.A. `fs::write` is fine for single-shot scratch files but never for files that other live processes read (`plan.yaml`, `.specify/plan.lock`, `.metadata.yaml`).

## CLI architecture

`src/cli.rs` declares the clap derive surface. Every command has a doc comment that doubles as `--help` output — keep it accurate and operator-facing (no internal jargon, no RFC numbers without a hyperlink). Add new commands as enum variants on `Commands` with a nested action enum where the verb has subactions; mirror existing groups (`SliceAction`, `ChangeAction`, etc.).

Dispatchers live in `src/commands/<verb>.rs` and call back into the workspace crates. The discipline is:

1. Clap parses argv → `Commands` enum.
2. `src/commands.rs` matches the variant and calls the dispatcher in `src/commands/<verb>.rs`.
3. The dispatcher loads `ProjectConfig` (which enforces the `specify_version` floor for free) and any other state it needs.
4. The dispatcher delegates the deterministic work to a workspace crate (`specify_slice`, `specify_change`, etc.) and converts the result to `CliResult`.

Never put domain logic in the binary. If a function needs unit tests, it belongs in a workspace crate. The binary owns argv parsing, formatting, and dispatch only.

## Layout boundary

`.specify/` is framework-managed state every CLI verb writes through (configuration under `project.yaml`, `slices/`, `archive/`, `.cache/`, `workspace/`, `plans/`, `plan.lock`). Operator-facing platform artifacts (`registry.yaml`, `plan.yaml`, `change.md`, `contracts/`) live at the repo root. The boundary is enforced by the `ProjectConfig::*_path` helpers in `src/config.rs` — every call site routes through them. Do not hard-code `.specify/registry.yaml` or sibling paths.

`detect_legacy_layout` and the `Error::LegacyLayout` ("`legacy-layout`") cutover gate any project-aware verb that tries to run on a v1-layout repo. Never silently read both layouts.

## Testing

Integration tests under `tests/` use `assert_cmd::Command::cargo_bin("specify")`, drive the binary through clap, and assert against stdout JSON or filesystem state. Test-binary names are `tests/<area>.rs` (`change_umbrella`, `cli`, `contract_tool`, `cross_repo`, `e2e`, `plan`, `slice`, `slice_merge`, `tool`, `vectis_tool`, `capability`).

Patterns to follow:

- Spin up a real `specify init` in a `tempfile::TempDir`. Reach for the existing helpers in `tests/cross_repo.rs` for multi-repo / fake-forge work; do not invent a parallel harness.
- Compare stdout JSON against checked-in goldens under `tests/fixtures/e2e/goldens/`. Regenerate with `REGENERATE_GOLDENS=1 cargo nextest run --test e2e` and `git diff` before committing. The harness substitutes tempdir paths to `<TEMPDIR>` so goldens stay machine-independent ([DECISIONS.md "Change J"](./DECISIONS.md#change-j--golden-json-generation-workflow)).
- Prefer structural assertions (status fields, exit codes, JSON shape) over byte-for-byte prose comparisons.
- Tests that need git operations set the four `GIT_*` env vars from `cross_repo.rs::GIT_TEST_ENV` so authorship is deterministic.

`tests/cross_repo.rs` is the RM-01 happy-path acceptance harness — read it first when extending multi-repo coverage.

## WASI tooling

`crates/contract-validate`, `crates/vectis-validate`, and `crates/vectis-scaffold` build for `wasm32-wasip2` and ship as WASI components. The `specify-tool` runner (`wasmtime` + `wasmtime-wasi`) loads them through `specify tool run <name>` per declared-tool permissions in `project.yaml.tools[]`. When editing these crates:

- They cannot use anything that isn't WASI-compatible. No threads, no networking primitives outside the declared WASI imports, no clock unless the manifest declares it.
- Rebuild artifacts via the `cargo make` recipes listed above. Do not check the `.wasm` outputs into git unless promoting a new release version (the release workflow handles distribution).
- Keep their crate dependency surface minimal — they ship as standalone components and bloat the WASM size if you pull in heavy crates.

## Supply chain

`cargo-vet`, `cargo-deny`, `cargo-audit`, `cargo-outdated`, and `cargo-udeps` all run in CI (`cargo make ci`). When a new dependency lands:

1. Add it to `[workspace.dependencies]` in the root `Cargo.toml` with a major-version pin (e.g. `serde = { version = "1", features = ["derive"] }`). Per-crate `Cargo.toml` references it as `serde.workspace = true`.
2. Run `cargo make vet` to regenerate the supply-chain audits, then commit the diff.
3. Check `deny.toml` allows the dependency's licence. The current allowlist is in `deny.toml`; add a new SPDX id only after confirming compatibility with MIT-OR-Apache-2.0.

Duplicate-version exemptions live in `clippy.toml` `allowed-duplicate-crates`. Add a new entry only when the duplicate is unavoidable (e.g. a transitive `windows-sys` major bump).

## Coding standards

### Comments

Comments answer "why does this look like this *today*?" — non-obvious intent, trade-offs, or constraints the code itself can't convey. RFC numbers, migration trails, and "this used to be X" rationale belong in `rfcs/`, `DECISIONS.md`, or commit messages — not in code or doc comments. Doc comments on items that surface in `--help` (clap `#[derive]` fields) must be operator-facing one-liners; rationale moves below the derive block where it doesn't leak into help output.

### Naming

Prefer short, idiomatic Rust names. Don't restate context the surrounding module, type, or function already supplies (`workspace::auto_commit_merge`, not `workspace_auto_commit_merge_baseline`). Avoid `_local` / `_value` / `_helper` suffixes. `is_kebab` over `is_valid_kebab_name`. New functions: 1–3 words. Public-API renames keep a deprecated alias for one release.

### Module layout

Use the modern Rust module layout: prefer `src/<parent>/<module>.rs` as the module entry point, with child modules under `src/<parent>/<module>/`. Do not add new `mod.rs` files inside module directories unless an external constraint requires it.

## Skill / CLI responsibility split (mirrors parent repo)

Every deterministic operation lives in this CLI: kebab-case validation, `.metadata.yaml` reads/writes, lifecycle transitions, capability resolution, artifact-completion checks, spec-merge preview, baseline conflict detection, delta merge, coherence validation, archive moves, plan/registry validation. The plugin repo's phase skills (`/spec:define`, `/spec:build`, `/spec:merge`, `/spec:drop`, `/spec:init`, `/change:plan`, `/change:execute`) shell out for all of those.

The corollary: when a skill currently does something deterministic in prose (parsing YAML, validating shape, computing topology, transitioning state), the right fix is to add a CLI verb here and have the skill call it. The wrong fix is to make the skill smarter.

## Gotchas

- Never hand-edit `.metadata.yaml` from a test or fixture. Drive transitions through `specify slice transition`, `specify slice outcome set`, `specify change plan transition`. The tests in `tests/slice.rs` are the canonical patterns.
- `specify init` bypasses the `specify_version` floor check (the file doesn't exist yet); every other project-aware verb inherits it for free via `ProjectConfig::load`. Don't reimplement the floor check at a subcommand site.
- `cargo nextest` and `cargo test` differ on `--no-tests=pass`. CI uses nextest with `--no-tests=pass`, so an empty test target is fine — but a missing `[[test]]` declaration that should exist will silently produce no output. Cross-check `cargo test` output if you suspect a target is being skipped.
- `cargo doc` is part of `cargo make ci`. Doc comments must compile. Reference paths inside backticks (`` `Self::config_path` ``) are fine; bare links (`[Foo]`) need a corresponding intra-doc target or rustdoc fails the build.
- Some workspace crates re-export types so the CLI can `use specify::{…}` instead of pulling each crate. Maintain those re-exports in `src/lib.rs` when adding a new public type that the CLI needs.

## Reference cross-links

- [DECISIONS.md](./DECISIONS.md) — running log of architectural decisions, indexed by RFC change letter. Read before changing error layering, exit codes, atomic writes, or YAML library choice.
- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [Parent repo `rfcs/`](https://github.com/augentic/specify/tree/main/rfcs) — active and archived RFCs. The CLI is the implementation surface for RFC-1, RFC-2, RFC-3a/b, RFC-9, RFC-13, RFC-14, RFC-15.
- `docs/release.md` — tagging and crates.io publish pipeline.
- `schemas/` — JSON Schema files distributed with the binary (`capability.schema.json`, `plan/`, `tool.schema.json`, `cache-meta.schema.json`).
