# Architecture

Workspace shape, crate dependency direction, the WASI carve-out, the `Layout<'a>` boundary, time injection, network hardening, and the rationale behind atomic writes. Read this before adding a new crate or shifting where state lives.

## Workspace layout

Binary crate (`name = "specify"`) at the repo root. `src/main.rs` is a thin `ExitCode` shim around `specify::run` defined in `src/lib.rs`; hosting the dispatch and command modules in a library lets workspace tooling (`xtask gen-man`, and future `gen-completions`) consume `specify::command()` (the `clap::Command` tree) without spawning the binary. New tooling that needs the command tree goes through `xtask`, not a parallel facade. Workspace member crates live under `crates/`; the dependency direction is leaf → root:

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-registry                 # depends on specify-error
specify-capability               # depends on specify-error
specify-task                     # depends on specify-error
specify-spec                     # leaf — no workspace deps (spec parser)
specify-tool                     # depends on specify-error (WASI tool runner; wasmtime)
specify-slice                    # depends on specify-{error,capability,registry}
specify-merge                    # depends on specify-{error,spec,capability,slice}
specify-config                   # depends on specify-{error,capability,slice,tool}
specify-validate                 # depends on specify-{error,spec,capability,registry,task}
specify-change                   # depends on specify-{error,config,registry,slice}
specify-init                     # depends on specify-{error,capability,config,registry}
specify (root crate)             # wires every workspace crate above into the CLI binary
```

Every crate uses the shared `[workspace.package]` (`edition = "2024"`, `rust-version = "1.93"`, MIT/Apache-2.0) and the shared `[workspace.lints]` block in the root `Cargo.toml` (clippy `all`/`cargo`/`nursery`/`pedantic` warned, plus a hand-picked `restriction` subset and a tightened rust lint set — `missing_debug_implementations`, `unreachable_pub`, `single_use_lifetimes`, `redundant_lifetimes`).

**Hard dependency rule:** `specify-error` is the leaf and depends on no other workspace crate. Adding a workspace dep to `specify-error` re-introduces the cycle the layering was designed to avoid; do not. The long-form rationale lives in [DECISIONS.md §"Error layering"](../../DECISIONS.md#error-layering).

**New workspace crates** are an exception, not the default. See [DECISIONS.md §"New workspace crates"](../../DECISIONS.md#new-workspace-crates) for the bar a new crate must clear.

The root `specify` crate has both `src/lib.rs` (the dispatcher) and `src/main.rs` (a thin `ExitCode` shim). New tooling that wants the clap command tree calls `specify::command()` through `xtask`; do **not** add a parallel binary or re-export.

## WASI carve-outs

WASI tools live in `wasi-tools/`, a sibling workspace excluded from the main lint posture. Members are `wasi-tools/contract` (`specify-contract`) and `wasi-tools/vectis` (`specify-vectis`). Build them by running `cargo build` inside `wasi-tools/` so the sibling workspace's lockfile and target dir are used — `cargo make contract-wasm` is a thin wrapper that does this for `specify-contract` and is required before running `tests/contract_tool.rs`; `scripts/build-vectis-local.sh` does the same for `specify-vectis` and adds sha256 sidecars for pre-release smoke tests.

`wasi-tools/contract` and `wasi-tools/vectis` are deliberate carve-outs from the workspace's Render/emit/`specify-error` discipline. They ship as standalone WASI components and live in their own sibling workspace at `wasi-tools/Cargo.toml`, which inherits a leaner lint posture and a minimal `[workspace.dependencies]` set. Do not pull `specify-error` (or any other host workspace crate that drags in `wasmtime`, `tokio`, `ureq`, …) into either; the carve-out comments in `wasi-tools/contract/src/main.rs` and `wasi-tools/vectis/src/lib.rs` are authoritative.

When editing these crates:

- They cannot use anything that isn't WASI-compatible. No threads, no networking primitives outside the declared WASI imports, no clock unless the manifest declares it.
- They stay outside the host workspace's Render/emit/`specify-error` discipline. Do not pull host workspace crates into either; `specify-validate` is the only path-dep bridge and it lives in `wasi-tools/Cargo.toml`'s `[workspace.dependencies]`.
- Rebuild artifacts from inside `wasi-tools/` so the sibling workspace's lockfile is used (`cargo make contract-wasm` and `scripts/build-vectis-local.sh` both do this). Do not check the `.wasm` outputs into git — the release workflow handles distribution.
- Keep their crate dependency surface minimal — they ship as standalone components and bloat the WASM size if you pull in heavy crates.

The `specify-tool` runner (`wasmtime` + `wasmtime-wasi`) loads them through `specify tool run <name>` per declared-tool permissions in `project.yaml.tools[]`.

## Layout boundary

`.specify/` is framework-managed state every CLI verb writes through (configuration under `project.yaml`, `slices/`, `archive/`, `.cache/`, `workspace/`, `plans/`, `plan.lock`). Operator-facing platform artifacts (`registry.yaml`, `plan.yaml`, `change.md`, `contracts/`) live at the repo root. The boundary is enforced by the `Layout<'a>` newtype in `specify-config` (`crates/config/src/lib.rs`): path helpers are inherent methods on `Layout<'a>`, and call sites write `Layout::new(&dir).plan_path()`. Do not hard-code `.specify/registry.yaml` or sibling paths, and do not declare free path-helper functions outside `crates/config/`; any new `.specify/` path lands on `Layout`.

## Time injection

Functions that record a timestamp into a serialised artifact accept `now: jiff::Timestamp` from the dispatcher boundary. Library crates do not call `Timestamp::now()`; the call site lives in `src/commands/*.rs` so tests can pin time deterministically. The current carve-out — `slice_actions::*` and friends still consume an injected `now` argument — is the canonical shape to follow.

## ureq fetch hardening

The WASI tool fetch in `crates/tool/src/resolver.rs` runs every HTTP request with explicit per-call timeouts, a `MAX_RESPONSE_BYTES` cap (64 MiB) checked on both the `Content-Length` header and the streamed body, and streams the response to a tempfile before persisting into the cache. Any new HTTP path that lands in this crate must adopt the same shape (timeouts + size cap + stream-to-tempfile); do not buffer arbitrary remote bodies into memory.

## Atomic writes

Use `yaml_write` (in `crates/slice/src/atomic.rs`) for any file a concurrent reader may observe mid-write: `plan.yaml`, `.metadata.yaml`, `journal.yaml`, `plan.lock`, and the registry. It serialises to `NamedTempFile::new_in(parent)` and `persist`-renames over the target so readers either see the prior bytes or the new bytes. Plain `fs::write` is reserved for files no other process reads concurrently with the writer (one-shot scratch output, fixtures inside a tempdir test).

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

Duplicate-version exemptions live in `clippy.toml` `allowed-duplicate-crates`. Add a new entry only when the duplicate is unavoidable (e.g. a transitive `windows-sys` major bump).

## Skill / CLI responsibility split

Every deterministic operation lives in this CLI: kebab-case validation, `.metadata.yaml` reads/writes, lifecycle transitions, capability resolution, artifact-completion checks, spec-merge preview, baseline conflict detection, delta merge, coherence validation, archive moves, plan/registry validation. The plugin repo's phase skills (`/spec:define`, `/spec:build`, `/spec:merge`, `/spec:drop`, `/spec:init`, `/change:plan`, `/change:execute`) shell out for all of those.

The corollary: when a skill currently does something deterministic in prose (parsing YAML, validating shape, computing topology, transitioning state), the right fix is to add a CLI verb here and have the skill call it. The wrong fix is to make the skill smarter.

The parent repo's [`AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) is the source of truth for workflow vocabulary (slice / change), skill family, plan-driven loop, and contract skills.
