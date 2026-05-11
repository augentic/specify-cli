# Specify CLI — Agent Instructions

This is a Rust workspace. It produces the `specify` binary that the [augentic/specify](https://github.com/augentic/specify) plugin repo's skills shell out to. Generated Rust crates and Swift shells produced by the workflow live in downstream consumer repositories; this repo owns the deterministic CLI primitives that those workflows compose.

## Workspace layout

Binary crate (`name = "specify"`, `[[bin]]`-only after CL-02) at the repo root, with workspace member crates under `crates/`. Dependency direction is leaf → root:

```text
specify-error                    # leaf — thiserror + serde-saphyr only
specify-registry                 # depends on specify-error
specify-capability               # depends on specify-error
specify-task                     # depends on specify-error
specify-spec                     # leaf — no workspace deps (spec parser)
specify-tool                     # depends on specify-error (WASI tool runner; wasmtime)
specify-slice                    # depends on specify-{error,capability,registry}
specify-merge                    # depends on specify-{error,spec,capability,slice}
specify-config                   # depends on specify-{error,capability,slice,tool}    (NEW from CL-01)
specify-validate                 # depends on specify-{error,spec,capability,registry,task}
specify-change                   # depends on specify-{error,config,registry,slice}
specify-init                     # depends on specify-{error,capability,config,registry} (NEW from CL-02)
specify (root crate)             # wires every workspace crate above into the CLI binary
crates/contract                  # standalone binary `specify-contract` (depends on specify-validate)
crates/vectis                    # standalone WASI component `specify-vectis` (validate + scaffold subcommands)
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
- `cargo make contract-wasm` / `vectis-wasm` / `vectis-wasi-artifacts` — build the WASI tool components for distribution.

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
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::CliTooOld` (`specify-version-too-old` in JSON) |

`CliResult::from(&Error)` in `src/output.rs` is the single source of truth. Every dispatcher in `src/commands/*` routes its terminal error through it. Do not invent new exit codes.

`unwrap()` and `expect()` are reserved for invariants the type system can't express (e.g. "this enum variant covers `Status::ALL`" — see `src/commands/status.rs`). Always include a justification string in `expect`. User-facing errors must surface as `Error::*` variants, not panics.

## YAML, JSON, and atomic writes

YAML (de)serialization goes through `serde-saphyr`, not `serde_yaml_ng` (retired) or `serde_yaml` (deprecated). `serde-saphyr` has no `Value` type; for dynamic YAML access deserialize into `serde_json::Value` (see [DECISIONS.md "YAML library"](./DECISIONS.md)). Errors split into `serde_saphyr::Error` (deser) and `serde_saphyr::ser::Error` (ser), and `specify-error::Error` carries both via `Yaml(#[from] …)` and `YamlSer(#[from] …)`.

Writes that must not be observed mid-update use the shared atomic helpers in `specify_slice::atomic` (`atomic_yaml_write` / `atomic_bytes_write`). `fs::write` is fine for single-shot scratch files but never for files that other live processes read (`plan.yaml`, `registry.yaml`, `change.md`, `tasks.md`, `.specify/plan.lock`, `.metadata.yaml`).

## CLI architecture

`src/cli.rs` declares the clap derive surface. Every command has a doc comment that doubles as `--help` output — keep it accurate and operator-facing (no internal jargon, no RFC numbers without a hyperlink). Add new commands as enum variants on `Commands` with a nested action enum where the verb has subactions; mirror existing groups (`SliceAction`, `ChangeAction`, etc.).

Dispatchers live in `src/commands/<verb>.rs` and call back into the workspace crates. The discipline is:

1. Clap parses argv → `Commands` enum.
2. `src/commands.rs` matches the variant and calls the dispatcher in `src/commands/<verb>.rs`.
3. The dispatcher loads `ProjectConfig` (which enforces the `specify_version` floor for free) and any other state it needs.
4. The dispatcher delegates the deterministic work to a workspace crate (`specify_slice`, `specify_change`, etc.) and converts the result to `CliResult`.

Never put domain logic in the binary. If a function needs unit tests, it belongs in a workspace crate. The binary owns argv parsing, formatting, and dispatch only.

## Layout boundary

`.specify/` is framework-managed state every CLI verb writes through (configuration under `project.yaml`, `slices/`, `archive/`, `.cache/`, `workspace/`, `plans/`, `plan.lock`). Operator-facing platform artifacts (`registry.yaml`, `plan.yaml`, `change.md`, `contracts/`) live at the repo root. The boundary is enforced by the `Layout<'a>` newtype in `specify-config` (`crates/config/src/lib.rs`): call sites write `dir.layout().plan_path()` (via the `LayoutExt` trait) or `Layout::new(&dir).plan_path()`. Do not hard-code `.specify/registry.yaml` or sibling paths.

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

`crates/contract` and `crates/vectis` build for `wasm32-wasip2` and ship as WASI components. The `specify-tool` runner (`wasmtime` + `wasmtime-wasi`) loads them through `specify tool run <name>` per declared-tool permissions in `project.yaml.tools[]`. When editing these crates:

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

### Naming

Prefer short, idiomatic Rust names. Don't restate context the surrounding module, type, or function already supplies. Avoid `_local` / `_value` / `_helper` suffixes. New functions: 1–3 words. Predicates start with `is_` / `has_`. DTOs returned by handlers are `<Action>Body` / `<Action>Row`, never `<Action>Response` / `<Action>Json` (the type's role is `Body`; the format dispatch lives in `emit`).

A function defined in `mod <name>` (or `commands/<name>.rs`) MUST NOT carry `<name>` as a suffix or prefix on its own name — the module path already supplies that context. The `name-suffix-duplication` predicate enforces this.

```rust
// BAD — file is commands/registry.rs / mod registry
fn show_registry(ctx: &Ctx) -> ... { ... }
fn validate_registry(ctx: &Ctx) -> ... { ... }
fn add_to_registry(ctx: &Ctx) -> ... { ... }

// GOOD — caller writes registry::show, registry::validate, registry::add
fn show(ctx: &Ctx) -> ... { ... }
fn validate(ctx: &Ctx) -> ... { ... }
fn add(ctx: &Ctx) -> ... { ... }

// BAD — overspecified DTO names
struct ShowRegistryResponse { /* ... */ }
struct ValidateRegistryJson { /* ... */ }

// GOOD
struct ShowBody { /* ... */ }
struct ValidateBody { /* ... */ }

// BAD
fn workspace_auto_commit_merge_baseline(...) { ... }
fn is_valid_kebab_name(s: &str) -> bool { ... }
fn parse_touched_spec_set_value(...) { ... }

// GOOD  (in mod workspace, mod kebab, mod touched_specs)
fn auto_commit_merge(...) { ... }
fn is_kebab(s: &str) -> bool { ... }
fn parse_set(...) { ... }
```

### Format dispatch

Handlers do **not** open-code `match ctx.format { Json, Text }`. Use the `Render` trait and the `emit` helper in [`src/output.rs`](./src/output.rs).

```rust
// BAD
match ctx.format {
    OutputFormat::Json => serde_json::to_writer(stdout(), &SomeBody { ... })?,
    OutputFormat::Text => println!("..."),
}

// GOOD
emit(Stream::Stdout, ctx.format, &SomeBody::from(result))?;
```

`Render::render_text(&self, w: &mut dyn Write)` carries the text-mode body; the JSON path goes through `serde::Serialize`. New code must not introduce `match … format`; the `format-match-dispatch` predicate fails new occurrences. See [`src/commands/codex.rs`](src/commands/codex.rs) for the canonical pattern.

### DTOs

Response DTOs (`*Body`, `*Row`) are **top-level** structs under `mod`. Inline DTOs trip the `inline-dtos` AST predicate (DTOs declared inside *any* `Block` — function bodies, match arms, closures — count) and force per-file `#![allow(items_after_statements, …)]` waivers. The waiver is itself a refactor signal: a file that needs it is a file whose handler hasn't been migrated yet.

```rust
// BAD — DTO inside fn body
fn handle(...) {
    #[derive(Serialize)]
    struct Body { name: String }
    emit(format, &Body { name })?;
}

// BAD — DTO inside match arm (the prior regex missed these; the AST
// predicate catches them)
match action {
    Action::List => {
        #[derive(Serialize)]
        struct ListRow { name: String }
        // ...
    }
    Action::Show { .. } => { /* ... */ }
}

// GOOD
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct HandleBody { name: String }

impl Render for HandleBody {
    fn render_text(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        writeln!(w, "{}", self.name)
    }
}

fn handle(...) {
    emit(format, &HandleBody { name })?;
}
```

### Errors

`specify-error::Error` variants are **structured**, not `Variant(String)` catch-alls. For new diagnostics, prefer in this order:

1. A dedicated typed variant (e.g. `Error::Argument`, `Error::PlanTransition`, `Error::ContextLockMalformed`) when the call shape recurs or carries structured payload.
2. `Error::Diag { code: "<kebab>", detail: format!(…) }` when it doesn't (yet). The `code` is the JSON envelope's stable `error` discriminant; `detail` is the human-readable message. Promote a recurring `Diag` site to its own variant once the call shape stabilises.

```rust
// GOOD — typed variant when the shape exists
return Err(Error::Argument {
    flag: "--proposed-name",
    detail: "required when outcome is `registry-amendment-required`".into(),
});

// GOOD — Diag when no typed variant fits yet
return Err(Error::Diag {
    code: "registry-amendment-required-needs-proposed-name",
    detail: "--proposed-name is required when outcome is `registry-amendment-required`".into(),
});
```

The kebab-case identifier in `#[error("…")]` (and in `Error::Diag.code`) is part of the public contract that skills and tests grep for; never rename without bumping `JSON_ENVELOPE_VERSION`.

### `#[non_exhaustive]`

Every public `enum` or `struct` that may grow gets `#[non_exhaustive]`. The exception is structurally complete types (`enum OutputFormat { Json, Text }`); document the choice in a doc-line. This keeps adding a variant from being a SemVer break.

### Deprecation cadence

Public-API renames keep at most **one** release of `#[deprecated]` aliases, then delete. Indefinite `// kept for legacy callers` is YAGNI debt — it metastasises (see the pre-2026 `schema-name` JSON envelope key, which lived as "transitional" for several releases).

### `#[allow]` posture

`#[allow(<lint>, reason = "…")]` lives at the **smallest scope** that fixes the lint. Identical `reason = "…"` strings across three or more files mean you should promote a single `#![allow]` to the parent module — the file-level repetition is noise, not signal.

```rust
// BAD — same allow/reason in every commands/*.rs
// (12 files, identical text)
#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to these command handlers."
)]

// GOOD — one allow at the parent
// src/commands.rs
#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to handlers in this module."
)]
```

### Module layout

Use the modern Rust module layout: prefer `src/<parent>/<module>.rs` as the module entry point, with child modules under `src/<parent>/<module>/`. Do not add new `mod.rs` files inside module directories unless an external constraint requires it.

### Mechanical enforcement

`cargo make standards-check` shells out to `cargo run -p xtask -- standards-check`. Predicates live in [`xtask/src/standards.rs`](xtask/src/standards.rs); per-file baselines live in [`scripts/standards-allowlist.toml`](scripts/standards-allowlist.toml). The xtask uses `syn` for AST predicates (so DTOs declared inside `match` arms count, where the prior regex missed them) and `regex` for textual predicates.

| Predicate | What it counts |
|---|---|
| `inline-dtos` | Structs/enums with `#[derive(Serialize)]` declared inside any `Block`. |
| `format-match-dispatch` | Hand-rolled `match … format { Json => … }`. Use `Render::render_text` + `emit` instead. |
| `rfc-numbers-in-code` | `RFC[- ]?\d+` outside `tests/`, `DECISIONS.md`, and `rfcs/`. |
| `ritual-doc-paragraphs` | The boilerplate `Returns an error if the operation fails.` doc paragraph. |
| `no-op-forwarders` | `let _ = cli.<flag>;` — a parsed-but-unused CLI flag. |
| `name-suffix-duplication` | `fn foo_<module>` inside `mod <module>` (e.g. `fn show_registry` in `commands/registry.rs`). |
| `currently-audit` | The word `Currently` in a clap-derive doc comment (`src/cli.rs` and `src/commands/**/cli.rs`). Catches the AGENTS.md `Wired-but-ignored flags` smell ("Currently equivalent to the default …") at PR time. |
| `error-envelope-inlined` | `output::ErrorBody { … }` / `output::ValidationErrBody { … }` constructed outside `src/output.rs`. Error envelopes are emitted via `report_error`, not hand-rolled at the call site. |
| `path-helper-inlined` | `fn specify_dir|plan_path|change_brief_path|archive_dir` declared outside `crates/config/`. Path helpers live in `specify-config`; command modules call them, they do not redefine them. Thin facade methods that take `&self` are exempted by the regex shape. |
| `ok-literal-in-body` | `pub ok: bool` field outside the carve-outs (`crates/validate/src/contracts/envelope.rs`, `crates/validate/src/compatibility/mod.rs`). The JSON envelope encodes success-vs-failure via the presence/absence of `error:`; the redundant `ok` field was removed in CL-E3 and this predicate keeps it gone. |
| `direct-fs-write` | Direct `fs::write` / `std::fs::write` in non-test Rust. Managed state must use the atomic helpers; any remaining scratch-only use needs an allowlist baseline and a comment. |
| `stale-cli-vocab` | Legacy CLI vocabulary in non-test Rust (`initiative`, `initiative.md`, retired top-level `specify plan`, `specify merge`, `specify validate`). Use `change`, `slice`, and the current command surface unless the file is an explicit historical record. |
| `module-line-count` | Non-test Rust source file length in lines. Default cap 500; per-file baselines grandfather oversized files until they are split. |

A live count strictly greater than its per-file baseline fails CI; missing predicates default to zero (new files start clean) except `module-line-count`, which defaults to 500.

**Ratchet** — any PR that touches a file with allowlist baselines is expected to reduce them where it can. CI runs `cargo run -p xtask -- standards-check --check-tightenable`, which fails when an unrelated PR could lower a baseline without code changes. Run `cargo make standards-tighten` and commit the updated `scripts/standards-allowlist.toml` to clear.

**Module length cap** — keep new modules ≤ 500 lines. When a file outgrows that, split by concern (one verb per file, model vs IO vs transitions, etc.) before adding more code. Prefer `src/<parent>/<module>.rs` + `src/<parent>/<module>/<concern>.rs` over a single fat file with `// ---` separators.

### No-op forwarders

A clap-parsed flag that is destructured and silently dropped (`let _ = cli.<flag>;` or pattern matches that never reach a handler) is a YAGNI smell. Either the flag is wired up (the variant carries data and the handler reads it) or it is removed from clap. The `no-op-forwarders` predicate fails new occurrences.

### Wired-but-ignored flags

Flag whose doc-comment says "Currently equivalent to the default …" or whose handler ignores the value is the same defect dressed up as documentation. Drop the flag from clap until the differentiated behaviour exists. The `currently-audit` predicate fails any new occurrence of the word `Currently` in a clap-derive doc comment under `src/cli.rs` or `src/commands/**/cli.rs`.

### Path helpers live in one crate

`.specify/` layout helpers (`specify_dir`, `plan_path`, `change_brief_path`, `archive_dir`, and friends) live in `specify-config` as inherent methods on the `Layout<'a>` newtype in [`crates/config/src/lib.rs`](crates/config/src/lib.rs). Call sites write `project_dir.layout().plan_path()` (via the `LayoutExt` trait on `&Path`) or `Layout::new(&project_dir).plan_path()` — they do not redefine their own copies. CL-01 hoisted these out of the binary's old `src/config.rs` so every workspace consumer routes through one source of truth, and R10 moved the helpers from associated functions on `ProjectConfig` to inherent methods on `Layout<'a>` so the `.specify/` boundary has one typed receiver. The `path-helper-inlined` predicate fails any new free-function `fn specify_dir|plan_path|change_brief_path|archive_dir` declared outside `crates/config/`.

### Error envelopes are not constructed in handlers

Handlers return `Result<T, specify_error::Error>` and let `report_error` in [`src/output.rs`](src/output.rs) shape the JSON wire envelope. Nobody constructs `output::ErrorBody { … }` or `output::ValidationErrBody { … }` by hand outside `src/output.rs` — the envelope shape (and its `error` discriminant contract) is owned there, and inlining it at a call site forks the contract. The `error-envelope-inlined` predicate (added in CL-X1) fails any such hand-rolled envelope outside `src/output.rs`.

### Quarterly migration cadence

**Quarterly migration cadence.** A scheduled PR — first business week of each quarter — reviews `scripts/standards-allowlist.toml`, identifies the top five files by total grandfathered violations, and either drives them to zero or documents in this section why they cannot be reduced this quarter. PR title: `chore: q<N> standards-allowlist sweep`.

See [`docs/contributing/maintenance.md`](docs/contributing/maintenance.md) for the operational playbook (picking targets, updating baselines, PR shape).

## Skill / CLI responsibility split (mirrors parent repo)

Every deterministic operation lives in this CLI: kebab-case validation, `.metadata.yaml` reads/writes, lifecycle transitions, capability resolution, artifact-completion checks, spec-merge preview, baseline conflict detection, delta merge, coherence validation, archive moves, plan/registry validation. The plugin repo's phase skills (`/spec:define`, `/spec:build`, `/spec:merge`, `/spec:drop`, `/spec:init`, `/change:plan`, `/change:execute`) shell out for all of those.

The corollary: when a skill currently does something deterministic in prose (parsing YAML, validating shape, computing topology, transitioning state), the right fix is to add a CLI verb here and have the skill call it. The wrong fix is to make the skill smarter.

## Gotchas

- Never hand-edit `.metadata.yaml` from a test or fixture. Drive transitions through `specify slice transition`, `specify slice outcome set`, `specify change plan transition`. The tests in `tests/slice.rs` are the canonical patterns.
- `specify init` bypasses the `specify_version` floor check (the file doesn't exist yet); every other project-aware verb inherits it for free via `ProjectConfig::load`. Don't reimplement the floor check at a subcommand site.
- `cargo nextest` and `cargo test` differ on `--no-tests=pass`. CI uses nextest with `--no-tests=pass`, so an empty test target is fine — but a missing `[[test]]` declaration that should exist will silently produce no output. Cross-check `cargo test` output if you suspect a target is being skipped.
- `cargo doc` is part of `cargo make ci`. Doc comments must compile. Reference paths inside backticks (`` `Self::config_path` ``) are fine; bare links (`[Foo]`) need a corresponding intra-doc target or rustdoc fails the build.
- The root `specify` crate is `[[bin]]`-only — there is no `src/lib.rs` (the legacy library shim that hosted local `config` and `init` modules was deleted by CL-02 once those modules moved to `specify-config` and `specify-init`). Public types from member crates are imported directly with `use specify_<crate>::Foo`; do **not** add a thin facade re-exporting them through a new `lib.rs`.

## Reference cross-links

- [DECISIONS.md](./DECISIONS.md) — running log of architectural decisions, indexed by RFC change letter. Read before changing error layering, exit codes, atomic writes, or YAML library choice.
- [Parent repo `AGENTS.md`](https://github.com/augentic/specify/blob/main/AGENTS.md) — workflow vocabulary (slice / change), skill family, plan-driven loop, contract skills.
- [Parent repo `rfcs/`](https://github.com/augentic/specify/tree/main/rfcs) — active and archived RFCs. The CLI is the implementation surface for RFC-1, RFC-2, RFC-3a/b, RFC-9, RFC-13, RFC-14, RFC-15.
- `docs/release.md` — tagging and crates.io publish pipeline.
- `schemas/` — JSON Schema files distributed with the binary (`capability.schema.json`, `plan/`, `tool.schema.json`, `cache-meta.schema.json`).
