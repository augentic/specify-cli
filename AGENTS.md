# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specify` binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives that those workflows compose.

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

WASI tools live in `wasi-tools/`, a sibling workspace excluded from the main lint posture. Members are `wasi-tools/contract` (`specify-contract`) and `wasi-tools/vectis` (`specify-vectis`). Build them through the `cargo make contract-wasm` / `vectis-wasm` recipes — those `cd wasi-tools` first so the sibling workspace's lockfile and target dir are used.

Every crate uses the shared `[workspace.package]` (`edition = "2024"`, `rust-version = "1.93"`, MIT/Apache-2.0) and the shared `[workspace.lints]` block in the root `Cargo.toml` (clippy `all`/`cargo`/`nursery`/`pedantic` warned, plus a hand-picked `restriction` subset and a tightened rust lint set — `missing_debug_implementations`, `unreachable_pub`, `single_use_lifetimes`, `redundant_lifetimes`).

Hard dependency rule: `specify-error` is the leaf and depends on no other workspace crate. Adding a workspace dep to `specify-error` re-introduces the cycle the layering was designed to avoid; do not.

**WASI carve-outs.** `wasi-tools/contract` and `wasi-tools/vectis` are deliberate carve-outs from the workspace's Render/emit/`specify-error` discipline. They ship as standalone WASI components and live in their own sibling workspace at `wasi-tools/Cargo.toml`, which inherits a leaner lint posture and a minimal `[workspace.dependencies]` set. Do not pull `specify-error` (or any other host workspace crate that drags in `wasmtime`, `tokio`, `ureq`, …) into either; the carve-out comments in `wasi-tools/contract/src/main.rs` and `wasi-tools/vectis/src/lib.rs` are authoritative.

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
- `cargo make standards-check` — runs the xtask predicates over the source tree (see §"Mechanical enforcement").
- `cargo make gen-man` — emits roff man pages for `specify` and every leaf subcommand into `target/man/` via `xtask gen-man`. Output is gitignored.
- `cargo make tools-test-fixtures` — rebuild WASI fixture components used by `tests/tool.rs`.
- `cargo make contract-wasm` / `vectis-wasm` / `vectis-wasi-artifacts` — build the WASI tool components for distribution.

Before committing, run the complete local CI suite with `cargo make ci` and fix any failures or warnings it surfaces. Do not rely on narrower substitutes such as `cargo test` or `cargo clippy`; if `cargo make ci` cannot be run, say exactly why and which checks were run instead.

## Lints

Workspace lints live in `Cargo.toml`. Defaults are aggressive — clippy `all`/`cargo`/`nursery`/`pedantic` are all `warn`, plus a curated set of `restriction` lints and a tightened rust lint set (`missing_debug_implementations`, `unreachable_pub`, `single_use_lifetimes`, `redundant_lifetimes`). Compile under `RUSTFLAGS=-Dwarnings` (`cargo make test` does this), so any new warning fails CI.

When you must silence a lint, use `#[expect(<lint>, reason = "…")]` at the **smallest possible scope**. `#[expect]` is preferred over `#[allow]` everywhere except module-level waivers: a dead `#[expect]` is a build failure, so the suppression cannot rot. `#![allow(...)]` at the crate or module root is still the right tool when the lint legitimately applies to every item below (e.g. `clippy::multiple_crate_versions` at the binary root). `clippy.toml` allows `GitHub`, `OAuth`, `OpenTelemetry`, `WebAssembly`, `YAML` as doc idents — extend it (not the surrounding doc comment) when a new proper noun trips `doc_markdown`.

`taplo.toml` formats `Cargo.toml` files. Dependency arrays under `*-dependencies` and `dependencies` reorder alphabetically; preserve that on edit.

## Error handling and exit codes

`specify-error::Error` is the only error type the CLI surfaces. Every fallible function returns `Result<T, specify_error::Error>` (often via the `pub type Result<T, E = Error>` alias re-exported as `specify_error::Result`). New error variants land in `crates/error/src/lib.rs` with a stable kebab-case identifier in the `#[error("…")]` message — those identifiers are part of the public contract that skills and tests grep for.

The four-slot CLI exit-code table is fixed:

| Code | Name | When |
|---|---|---|
| 0 | `EXIT_SUCCESS` | Command succeeded |
| 1 | `EXIT_GENERIC_FAILURE` | Default `Error` → exit 1 |
| 2 | `EXIT_VALIDATION_FAILED` | `Error::Validation`, undeclared/over-permissioned tool, `Error::Argument` |
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::CliTooOld` (`specify-version-too-old` in JSON) |

`Exit::from(&Error)` in `src/output.rs` is the single source of truth. Every dispatcher in `src/commands/*` routes its terminal error through `report`, which calls `Exit::from`. Do not invent new exit codes. `Exit::Code(u8)` is a WASI passthrough used by `specify tool run` to forward a WASI guest exit verbatim; it is not for ad-hoc subcommand use.

`unwrap()` and `expect()` are reserved for invariants the type system can't express (e.g. "this enum variant covers `Status::ALL`"). Always include a justification string in `expect`. User-facing errors must surface as `Error::*` variants, not panics.

## Handler shape

Command handlers default to `Result<()>` (success-path conversion happens at the dispatcher boundary). Surface non-success exits through typed errors that `Exit::from(&Error)` maps to the four-slot exit table — do **not** return `Result<Exit>` to thread a non-zero code by hand.

Handlers take `&Ctx` (renamed from `CommandContext` so the module path `crate::context::Ctx` carries the noun). `Ctx` exposes the resolved project dir, layout, output format, and a few thin facade methods for handler ergonomics; everything else flows through workspace crates.

```rust
// GOOD — default shape
pub(crate) fn handle(ctx: &Ctx, args: &SomeArgs) -> Result<()> {
    let body = some_crate::do_work(ctx.layout(), args)?;
    ctx.out().write(&SomeBody::from(&body))?;
    Ok(())
}

// GOOD — explicit Result<Exit> only when the handler needs a
// non-success exit and a typed *ErrBody (rare — workspace::push is one).
pub(crate) fn handle(ctx: &Ctx) -> Result<Exit, Error> { /* ... */ }
```

A free `fn ... -> Result<Exit>` declared outside `src/commands.rs` trips the `result-cliresult-default` predicate; the surviving carve-outs are listed in `scripts/standards-allowlist.toml` and shrink as handlers are migrated.

## YAML, JSON, and atomic writes

YAML (de)serialization goes through `serde-saphyr`, not `serde_yaml_ng` (retired) or `serde_yaml` (deprecated). `serde-saphyr` has no `Value` type; for dynamic YAML access deserialize into `serde_json::Value`. Deser and ser errors are wrapped behind `specify_error::YamlError` / `specify_error::YamlSerError` so the upstream crate name does not leak through every `specify-*` public surface; `specify_error::Error` carries both via `Yaml(#[from] YamlError)` and `YamlSer(#[from] YamlSerError)`, and `?` on a raw `serde_saphyr` result still propagates because `Error` also implements `From<serde_saphyr::Error>` and `From<serde_saphyr::ser::Error>` through the wrappers. Library crates use the wrapper types in their public signatures; never expose `serde_saphyr::*::Error` directly.

Writes that must not be observed mid-update use the shared atomic helpers in `specify_slice::atomic` (`yaml_write` / `bytes_write`). `fs::write` is fine for single-shot scratch files but never for files that other live processes read (`plan.yaml`, `registry.yaml`, `change.md`, `tasks.md`, `.specify/plan.lock`, `.metadata.yaml`). The `direct-fs-write` predicate fails any new `fs::write` / `std::fs::write` in non-test Rust.

## CLI architecture

`src/cli.rs` declares the clap derive surface. Every command has a doc comment that doubles as `--help` output — keep it accurate and operator-facing (no internal jargon, no RFC numbers without a hyperlink). Add new commands as enum variants on `Commands` with a nested action enum where the verb has subactions; mirror existing groups (`SliceAction`, `ChangeAction`, etc.).

`--source key=value` arguments are parsed via the typed `SourceArg` (`impl FromStr for SourceArg`) so call sites read named fields instead of tuple positions.

Dispatchers live in `src/commands/<verb>.rs` and call back into the workspace crates. The discipline is:

1. Clap parses argv → `Commands` enum.
2. `src/commands.rs` matches the variant and calls the dispatcher in `src/commands/<verb>.rs`.
3. The dispatcher loads `ProjectConfig` (which enforces the `specify_version` floor for free) and any other state it needs.
4. The dispatcher delegates the deterministic work to a workspace crate (`specify_slice`, `specify_change`, etc.) and converts the result to a `*Body` for `ctx.out().write(...)`.

Never put domain logic in the binary. If a function needs unit tests, it belongs in a workspace crate. The binary owns argv parsing, formatting, and dispatch only.

## Layout boundary

`.specify/` is framework-managed state every CLI verb writes through (configuration under `project.yaml`, `slices/`, `archive/`, `.cache/`, `workspace/`, `plans/`, `plan.lock`). Operator-facing platform artifacts (`registry.yaml`, `plan.yaml`, `change.md`, `contracts/`) live at the repo root. The boundary is enforced by the `Layout<'a>` newtype in `specify-config` (`crates/config/src/lib.rs`): path helpers are inherent methods on `Layout<'a>`, and call sites write `dir.layout().plan_path()` (via the `LayoutExt` trait on `&Path`) or `Layout::new(&dir).plan_path()`. Do not hard-code `.specify/registry.yaml` or sibling paths, and do not declare free path-helper functions outside `crates/config/`; any new `.specify/` path lands on `Layout`. The `path-helper-inlined` predicate enforces this.

## Time injection

Functions that record a timestamp into a serialised artifact accept `now: chrono::DateTime<Utc>` from the dispatcher boundary. Library crates do not call `Utc::now()`; the call site lives in `src/commands/*.rs` so tests can pin time deterministically. The current carve-out — `slice_actions::*` and friends still consume an injected `now` argument — is the canonical shape to follow.

## ureq fetch hardening

The WASI tool fetch in `crates/tool/src/resolver.rs` runs every HTTP request with explicit per-call timeouts, a `MAX_RESPONSE_BYTES` cap (64 MiB) checked on both the `Content-Length` header and the streamed body, and streams the response to a tempfile before persisting into the cache. Any new HTTP path that lands in this crate must adopt the same shape (timeouts + size cap + stream-to-tempfile); do not buffer arbitrary remote bodies into memory.

## Testing

Integration tests under `tests/` use `assert_cmd::Command::cargo_bin("specify")`, drive the binary through clap, and assert against stdout JSON or filesystem state. Test-binary names are `tests/<area>.rs` (`change_umbrella`, `cli`, `contract_tool`, `cross_repo`, `e2e`, `plan`, `slice`, `slice_merge`, `tool`, `vectis_tool`, `capability`).

One file per integration binary is the intentional layout — `tests/it.rs` consolidation was measured and dropped, see [DECISIONS.md "Integration tests"](./DECISIONS.md#integration-tests--keep-per-file-binaries-no-testsitrs-umbrella). The cold-build win was 7.3 % cargo-reported (well below the 20 % bar we apply to "Idiomatic Rust Cleanup" chunks) and the per-binary split keeps `cargo test --test <area>` cheap for local iteration.

Patterns to follow:

- Spin up a real `specify init` in a `tempfile::TempDir`. Reach for the existing helpers in `tests/cross_repo.rs` for multi-repo / fake-forge work; do not invent a parallel harness.
- Compare stdout JSON against checked-in goldens under `tests/fixtures/e2e/goldens/`. Regenerate with `REGENERATE_GOLDENS=1 cargo nextest run --test e2e` and `git diff` before committing. The harness substitutes tempdir paths to `<TEMPDIR>` so goldens stay machine-independent.
- Prefer structural assertions (status fields, exit codes, JSON shape) over byte-for-byte prose comparisons.
- Tests that need git operations set the four `GIT_*` env vars from `tests/common::GIT_ENV` so authorship is deterministic.

`tests/cross_repo.rs` is the RM-01 happy-path acceptance harness — read it first when extending multi-repo coverage.

## WASI tooling

`wasi-tools/contract` and `wasi-tools/vectis` build for `wasm32-wasip2` and ship as WASI components from the sibling workspace at `wasi-tools/Cargo.toml`. The `specify-tool` runner (`wasmtime` + `wasmtime-wasi`) loads them through `specify tool run <name>` per declared-tool permissions in `project.yaml.tools[]`. When editing these crates:

- They cannot use anything that isn't WASI-compatible. No threads, no networking primitives outside the declared WASI imports, no clock unless the manifest declares it.
- They stay outside the host workspace's Render/emit/`specify-error` discipline (see "WASI carve-outs" above). Do not pull host workspace crates into either; `specify-validate` is the only path-dep bridge and it lives in `wasi-tools/Cargo.toml`'s `[workspace.dependencies]`.
- Rebuild artifacts via the `cargo make` recipes listed above (each one `cd`s into `wasi-tools/` so the sibling workspace's lockfile is used). Do not check the `.wasm` outputs into git unless promoting a new release version (the release workflow handles distribution).
- Keep their crate dependency surface minimal — they ship as standalone components and bloat the WASM size if you pull in heavy crates.

## Supply chain

`cargo-vet`, `cargo-deny`, `cargo-audit`, `cargo-outdated`, and `cargo-udeps` all run in CI (`cargo make ci`). When a new dependency lands:

1. Add it to `[workspace.dependencies]` in the root `Cargo.toml` with a major-version pin (e.g. `serde = { version = "1", features = ["derive"] }`). Per-crate `Cargo.toml` references it as `serde.workspace = true`.
2. Run `cargo make vet` to regenerate the supply-chain audits, then commit the diff.
3. Check `deny.toml` allows the dependency's licence. The current allowlist is in `deny.toml`; add a new SPDX id only after confirming compatibility with MIT-OR-Apache-2.0.

Duplicate-version exemptions live in `clippy.toml` `allowed-duplicate-crates`. Add a new entry only when the duplicate is unavoidable (e.g. a transitive `windows-sys` major bump).

## Coding standards

These rules are enforced by `cargo make standards-check` (run in CI) and by review. CI failure messages cite this section by anchor (e.g. `see AGENTS.md#comments`). When a rule fights you, add the case to the rule with a before/after — don't carve out a local exception.

### Comments

Comments answer "why does this look like this *today*?" — non-obvious intent, trade-offs, or constraints the code itself can't convey. RFC numbers, migration trails, and "this used to be X" rationale belong in `rfcs/`, `DECISIONS.md`, or commit messages — not in code or doc comments. Doc comments on items that surface in `--help` (clap `#[derive]` fields) must be operator-facing one-liners; rationale moves below the derive block where it doesn't leak into help output.

```rust
// BAD
//! Per RFC-13 chunk 2.9 ("Init wires components, not capabilities"),
//! `init` writes only the per-project skeleton — `project.yaml` plus
//! the `.specify/` tree. The pre-Phase-3.7 filename was `initiative.md`;
//! RFC-13 chunk 3.7 renamed it to `change.md` …

// GOOD
//! Scaffolds `.specify/` plus `project.yaml`. Operator-facing artifacts
//! (`registry.yaml`, `change.md`, `plan.yaml`) are minted by their
//! owning verbs, not by `init`.
```

Doc comments describe what this is today. Version-history tables, dated bumps, commit hashes, and migration notes belong in git log or `DECISIONS.md` — not in `///` blocks. Doc paragraphs over 8 consecutive non-blank lines on a `pub` item are flagged by `verbose-doc-paragraphs`.

### Naming

Prefer short, idiomatic Rust names. Don't restate context the surrounding module, type, or function already supplies. Avoid `_local` / `_value` / `_helper` suffixes. New functions: 1–3 words. Predicates start with `is_` / `has_`. DTOs returned by handlers are `<Action>Body` / `<Action>Row`, never `<Action>Response` / `<Action>Json` (the type's role is `Body`; the format dispatch lives in `emit`).

A function defined in `mod <name>` (or `commands/<name>.rs`) MUST NOT carry `<name>` as a suffix or prefix on its own name — the module path already supplies that context. Clippy's `module_name_repetitions` (on by default through the `pedantic` group) catches this at lint time.

```rust
// BAD — file is commands/registry.rs / mod registry
fn show_registry(ctx: &Ctx) -> ... { ... }
fn validate_registry(ctx: &Ctx) -> ... { ... }
fn add_to_registry(ctx: &Ctx) -> ... { ... }

// GOOD — caller writes registry::show, registry::validate, registry::add
fn show(ctx: &Ctx) -> ... { ... }
fn validate(ctx: &Ctx) -> ... { ... }
fn add(ctx: &Ctx) -> ... { ... }
```

### Format dispatch

Handlers do **not** open-code `match ctx.format { Json, Text }`. There is one entry point — `ctx.out().write(&SomeBody::from(&result))` for success bodies, and `report(ctx.format, &err)` (which dispatches `ErrorBody` / `ValidationErrBody` to `Stream::Stderr`) for failures. `Stream::Stdout` / `Stream::Stderr` and the underlying `emit` function are private to `src/output.rs`; handlers never spell them. `emit_err` / `emit_response` / `emit_error` / `emit_json_error` have all been collapsed into this single surface.

```rust
// BAD
match ctx.format {
    Format::Json => serde_json::to_writer(stdout(), &SomeBody::from(&r))?,
    Format::Text => println!("..."),
}

// GOOD
ctx.out().write(&SomeBody::from(&result))?;
```

Format-only handlers that run before (or outside of) a `Ctx` — `commands::init::run`, `commands::capability::resolve`, `commands::capability::check` — receive a bare `Format` and reach for `Out::for_format(format).write(&Body)?;` instead.

`Render::render_text(&self, w: &mut dyn Write)` carries the text-mode body; the JSON path goes through `serde::Serialize`. New code must not introduce `match … format`; the `format-match-dispatch` predicate fails new occurrences. See [`src/commands/codex.rs`](src/commands/codex.rs) for the canonical pattern.

### One emit path

Success bodies leave handlers via `ctx.out().write(&Body)?;` (or `Out::for_format(format).write(&Body)?;` for the rare `Ctx`-less verb). Failure envelopes leave handlers as `Err(Error::*)`; the dispatcher in `src/commands.rs` routes them through `output::report(format, &err)`. No handler emits its own `Stream::Stderr` envelope. If you need a bespoke failure shape, add an `Error` variant with a kebab-case discriminant; do not hand-roll a `*ErrBody` DTO. `Stream` and `emit` are private to `src/output.rs` and stay that way.

### DTOs

Response DTOs (`*Body`, `*Row`) are **top-level** structs under `mod`. Inline DTOs trip the `inline-dtos` AST predicate (DTOs declared inside *any* `Block` — function bodies, match arms, closures — count) and force per-file `#![allow(items_after_statements, …)]` waivers. The waiver is itself a refactor signal: a file that needs it is a file whose handler hasn't been migrated yet.

**Construct DTOs through `From` impls, not named builders.** Use `impl From<&Domain> for Body` so the conversion is discoverable at the trait surface and call sites read `Body::from(&domain)`. Named constructors are reserved for multi-arg or fallible builders (e.g. `RegistryProposalRow::from_kind` returns `Option<Self>`); each survivor carries a one-line doc justification.

**Typed fields, not stringly-typed ones.** `pub status` / `pub kind` (and any other field whose domain has a finite enum) carry the underlying domain enum with `#[derive(Serialize)]` + `#[serde(rename_all = "kebab-case")]`. Drop `.to_string()` at construction sites; the wire shape is unchanged.

**`PathBuf` for path fields, with `serialize_path`.** `*Body` fields that hold a filesystem path are `path: PathBuf`, serialised through `#[serde(serialize_with = "crate::output::serialize_path")]` (the helper falls back when `canonicalize` fails). Do not store `String` paths in DTOs.

```rust
// BAD — DTO inside fn body
fn handle(...) {
    #[derive(Serialize)]
    struct Body { name: String }
    emit(Stream::Stdout, format, &Body { name })?;
}

// BAD — named builder, stringly-typed status, String path
impl Body {
    pub(crate) fn from_outcome(outcome: &Outcome, path: PathBuf) -> Self {
        Self {
            status: outcome.status.to_string(),
            path: path.display().to_string(),
        }
    }
}

// GOOD
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct HandleBody {
    name: String,
    status: OutcomeStatus,
    #[serde(serialize_with = "crate::output::serialize_path")]
    path: PathBuf,
}

impl Render for HandleBody {
    fn render_text(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        writeln!(w, "{}", self.name)
    }
}

impl From<&Outcome> for HandleBody {
    fn from(outcome: &Outcome) -> Self { /* ... */ }
}

fn handle(ctx: &Ctx, outcome: &Outcome) -> Result<()> {
    ctx.out().write(&HandleBody::from(outcome))?;
    Ok(())
}
```

### Errors

`specify-error::Error` variants are **structured**, not `Variant(String)` catch-alls. For new diagnostics, prefer in this order:

1. A dedicated typed variant (e.g. `Error::Argument`, `Error::PlanTransition`, `Error::ContextLockMalformed`) when the call shape recurs or carries structured payload.
2. `Error::Diag { code: "<kebab>", detail: format!(…) }` when it doesn't (yet). The `code` is the JSON envelope's stable `error` discriminant; `detail` is the human-readable message. Promote a recurring `Diag` site to its own variant once the call shape stabilises.

The kebab-case identifier in `#[error("…")]` (and in `Error::Diag.code`) is part of the public contract that skills and tests grep for; never rename without bumping `ENVELOPE_VERSION`.

### `#[non_exhaustive]`

Every public `enum` or `struct` that may grow gets `#[non_exhaustive]`. The exception is structurally complete types (`enum Format { Json, Text }`); document the choice in a doc-line. This keeps adding a variant from being a SemVer break.

### Lint suppression posture

Site-local suppressions are `#[expect(<lint>, reason = "…")]`, not `#[allow]` — a dead `#[expect]` is a build failure, so the suppression cannot rot. Module-level waivers stay `#![allow(<lint>, reason = "…")]` because lint-rot detection at the module root is not useful (the waiver typically covers many sites). Identical `reason = "…"` strings across three or more files mean you should promote a single `#![allow]` to the parent module — the file-level repetition is noise, not signal.

```rust
// BAD — site-local #[allow]
#[allow(clippy::cognitive_complexity, reason = "linear state machine")]
fn step(...) { ... }

// GOOD — same scope, #[expect]
#[expect(clippy::cognitive_complexity, reason = "linear state machine")]
fn step(...) { ... }

// GOOD — repeated waiver hoisted to the module root
// src/commands.rs
#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to handlers in this module."
)]
```

### Module layout

Use the modern Rust module layout: prefer `src/<parent>/<module>.rs` as the module entry point, with child modules under `src/<parent>/<module>/`. Do not add new `mod.rs` files inside module directories unless an external constraint requires it.

### Drift audit

When you remove a symbol, run `rg <SymbolName> -- AGENTS.md DECISIONS.md` and update every hit in the same PR. Stale symbol references in docs are worse than missing docs — they teach the reader something false. The `stale-cli-vocab` predicate catches retired CLI nouns; doc drift on internal symbols (error variants, type names, field keys) is caught only by this audit habit.

### Mechanical enforcement

`cargo make standards-check` shells out to `cargo run -p xtask -- standards-check`. Predicates live in [`xtask/src/standards.rs`](xtask/src/standards.rs); per-file baselines live in [`scripts/standards-allowlist.toml`](scripts/standards-allowlist.toml). The xtask uses `syn` for AST predicates (so DTOs declared inside `match` arms count, where the prior regex missed them) and `regex` for textual predicates.

| Predicate | What it counts |
|---|---|
| `cli-help-shape` | Clap-derive `///` doc lines longer than 80 characters in `src/cli.rs` and `src/commands/**/cli.rs`. Help output is operator-facing and wraps poorly past 80 columns. |
| `direct-fs-write` | Direct `fs::write` / `std::fs::write` in non-test Rust. Managed state must use the atomic helpers. |
| `error-envelope-inlined` | `output::ErrorBody { … }` / `output::ValidationErrBody { … }` constructed outside `src/output.rs`. Error envelopes are emitted via `report`, not hand-rolled at the call site. |
| `format-match-dispatch` | Hand-rolled `match … format { Json => … }`. Use `Render::render_text` + `emit` instead. |
| `inline-dtos` | Structs/enums with `#[derive(Serialize)]` declared inside any `Block`. |
| `module-line-count` | Non-test Rust source file length in lines. Default cap 400; per-file baselines grandfather oversized files until they are split. |
| `no-op-forwarders` | `let _ = cli.<flag>;` — a parsed-but-unused CLI flag. |
| `path-helper-inlined` | `fn specify_dir|plan_path|change_brief_path|archive_dir` declared outside `crates/config/`. Path helpers live on `Layout<'a>` in `specify-config`. |
| `result-cliresult-default` | Free `fn ... -> Result<Exit>` outside `src/commands.rs`. New handlers default to `Result<()>` and let the dispatcher collapse the success path; surviving carve-outs are grandfathered. |
| `rfc-numbers-in-code` | `RFC[- ]?\d+` outside `tests/`, `DECISIONS.md`, and `rfcs/`. |
| `ritual-doc-paragraphs` | The boilerplate `Returns an error if the operation fails.` doc paragraph. |
| `stale-cli-vocab` | Legacy CLI vocabulary in non-test Rust (`initiative`, `initiative.md`, retired top-level `specify plan`, `specify merge`, `specify validate`). Use `change`, `slice`, and the current command surface. |
| `verbose-doc-paragraphs` | A `///` doc paragraph longer than 8 consecutive non-blank lines on a `pub fn|struct|enum|const|type`. Long prose belongs in `rfcs/` or `DECISIONS.md`. `pub trait` is exempt. |

A live count strictly greater than its per-file baseline fails CI; missing predicates default to zero (new files start clean) except `module-line-count`, which defaults to 400.

**Ratchet** — any PR that touches a file with allowlist baselines is expected to reduce them where it can. CI runs `cargo run -p xtask -- standards-check --check-tightenable`, which fails when an unrelated PR could lower a baseline without code changes. Run `cargo make standards-tighten` and commit the updated `scripts/standards-allowlist.toml` to clear.

**Module length cap** — keep new modules ≤ 400 lines. When a file outgrows that, split by concern (one verb per file, model vs IO vs transitions, etc.) before adding more code. Prefer `src/<parent>/<module>.rs` + `src/<parent>/<module>/<concern>.rs` over a single fat file with `// ---` separators.

### No-op forwarders

A clap-parsed flag that is destructured and silently dropped (`let _ = cli.<flag>;` or pattern matches that never reach a handler) is a YAGNI smell. Either the flag is wired up (the variant carries data and the handler reads it) or it is removed from clap. The `no-op-forwarders` predicate fails new occurrences.

### Wired-but-ignored flags

A flag whose doc-comment says "Currently equivalent to the default …" or whose handler ignores the value is the same defect as `no-op-forwarders` dressed up as documentation. Drop the flag from clap until the differentiated behaviour exists.

## Skill / CLI responsibility split (mirrors parent repo)

Every deterministic operation lives in this CLI: kebab-case validation, `.metadata.yaml` reads/writes, lifecycle transitions, capability resolution, artifact-completion checks, spec-merge preview, baseline conflict detection, delta merge, coherence validation, archive moves, plan/registry validation. The plugin repo's phase skills (`/spec:define`, `/spec:build`, `/spec:merge`, `/spec:drop`, `/spec:init`, `/change:plan`, `/change:execute`) shell out for all of those.

The corollary: when a skill currently does something deterministic in prose (parsing YAML, validating shape, computing topology, transitioning state), the right fix is to add a CLI verb here and have the skill call it. The wrong fix is to make the skill smarter.

## Gotchas

- Never hand-edit `.metadata.yaml` from a test or fixture. Drive transitions through `specify slice transition`, `specify slice outcome set`, `specify change plan transition`. The tests in `tests/slice.rs` are the canonical patterns.
- `specify init` bypasses the `specify_version` floor check (the file doesn't exist yet); every other project-aware verb inherits it for free via `ProjectConfig::load`. Don't reimplement the floor check at a subcommand site.
- `cargo nextest` and `cargo test` differ on `--no-tests=pass`. CI uses nextest with `--no-tests=pass`, so an empty test target is fine — but a missing `[[test]]` declaration that should exist will silently produce no output. Cross-check `cargo test` output if you suspect a target is being skipped.
- `cargo doc` is part of `cargo make ci`. Doc comments must compile. Reference paths inside backticks (`` `Self::config_path` ``) are fine; bare links (`[Foo]`) need a corresponding intra-doc target or rustdoc fails the build.
- The root `specify` crate has both `src/lib.rs` (the dispatcher) and `src/main.rs` (a thin `ExitCode` shim). New tooling that wants the clap command tree calls `specify::command()` through `xtask`; do **not** add a parallel binary or re-export.

## Reference cross-links

- [DECISIONS.md](./DECISIONS.md) — running log of architectural decisions. Read before changing error layering, exit codes, atomic writes, or YAML library choice.
- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [Parent repo `rfcs/`](https://github.com/augentic/specify/tree/main/rfcs) — active and archived RFCs. The CLI is the implementation surface for RFC-1, RFC-2, RFC-3a/b, RFC-9, RFC-13, RFC-14, RFC-15.
- `docs/release.md` — tagging and crates.io publish pipeline.
- `schemas/` — JSON Schema files distributed with the binary (`capability.schema.json`, `plan/`, `tool.schema.json`, `cache-meta.schema.json`).
