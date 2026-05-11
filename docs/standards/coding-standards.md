# Coding standards

Style rules every Rust change in this workspace honours. These rules are enforced by `cargo make standards` (run in CI) and by review; the mechanical predicates that back them are catalogued in [predicates.md](./predicates.md). CI failure messages cite this document by anchor. When a rule fights you, add the case to the rule with a before/after — don't carve out a local exception.

## Lints

Workspace lints live in `Cargo.toml`. Defaults are aggressive — clippy `all`/`cargo`/`nursery`/`pedantic` are all `warn`, plus a curated set of `restriction` lints and a tightened rust lint set (`missing_debug_implementations`, `unreachable_pub`, `single_use_lifetimes`, `redundant_lifetimes`). Compile under `RUSTFLAGS=-Dwarnings` (`cargo make test` does this), so any new warning fails CI.

When you must silence a lint, use `#[expect(<lint>, reason = "…")]` at the **smallest possible scope**. `#[expect]` is preferred over `#[allow]` everywhere except module-level waivers: a dead `#[expect]` is a build failure, so the suppression cannot rot. `#![allow(...)]` at the crate or module root is still the right tool when the lint legitimately applies to every item below (e.g. `clippy::multiple_crate_versions` at the binary root). `clippy.toml` allows `GitHub`, `OAuth`, `OpenTelemetry`, `WebAssembly`, `YAML` as doc idents — extend it (not the surrounding doc comment) when a new proper noun trips `doc_markdown`.

`taplo.toml` formats `Cargo.toml` files. Dependency arrays under `*-dependencies` and `dependencies` reorder alphabetically; preserve that on edit.

## Lint suppression posture

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

## Comments

Comments answer "why does this look like this *today*?" — non-obvious intent, trade-offs, or constraints the code itself can't convey. RFC numbers, migration trails, and "this used to be X" rationale belong in `rfcs/`, [DECISIONS.md](../../DECISIONS.md), or commit messages — not in code or doc comments. Doc comments on items that surface in `--help` (clap `#[derive]` fields) must be operator-facing one-liners; rationale moves below the derive block where it doesn't leak into help output.

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

Doc comments describe what this is today. Version-history tables, dated bumps, commit hashes, and migration notes belong in git log or [DECISIONS.md](../../DECISIONS.md) — not in `///` blocks. Doc paragraphs over 8 consecutive non-blank lines on a `pub` item are flagged by `verbose-doc-paragraphs` (see [predicates.md](./predicates.md)).

`cargo doc` is part of `cargo make ci`, so doc comments must compile. Reference paths inside backticks (`` `Self::config_path` ``) are fine; bare links (`[Foo]`) need a corresponding intra-doc target or rustdoc fails the build.

## Naming

Prefer short, idiomatic Rust names. Don't restate context the surrounding module, type, or function already supplies. Avoid `_local` / `_value` / `_helper` suffixes. New functions: 1–3 words. Predicates start with `is_` / `has_`. DTOs returned by handlers are `<Action>Body` / `<Action>Row`, never `<Action>Response` / `<Action>Json` (the type's role is `Body`; the format dispatch lives in `emit` — see [handler-shape.md](./handler-shape.md)).

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

## Brevity

The codebase optimises for short reading over short writing. Concretely:

- **Names**: 1–3 words. Predicates start with `is_` / `has_`. Avoid `_local` / `_value` / `_helper` / `_path` / `_dir` suffixes when the parameter type or surrounding context already says so (`is_workspace_clone(p: &Path)`, not `is_workspace_clone_path`).
- **Cross-module redundancy**: `WorkspaceBranchPreparationFailed` inside `Error` reads as `Error::WorkspaceBranchPreparationFailed` — drop the `Workspace` prefix when every variant in the cluster already operates on a workspace. Clippy's `module_name_repetitions` catches the in-module cases; cross-module redundancy is on you and reviewers.
- **One-variant enums** are dead overhead. Drop the variant or the enum. If the type's name already discriminates, the enum adds nothing.
- **Field prefixes**: a struct named `RegistryAmendmentArgs` does not carry `proposed_` on every field — the struct name already says "proposal".
- **Comment redundancy**: don't paraphrase a `match` arm's variant in a `// …` comment when the variant's doc-comment already explains it. The same rule applies to `Exit::code()`'s inline comments mirroring variant docs.

`verbose-doc-paragraphs` (8-line cap on `pub` items) and `ritual-doc-paragraphs` (boilerplate "Returns an error if …") catch the mechanical cases. Brevity at the type, field, and variant level is on you.

## Format dispatch

Handlers do **not** open-code `match ctx.format { Json, Text }`. There is one entry point — `ctx.out().write(&SomeBody::from(&result))` for success bodies, and `report(ctx.format, &err)` (which dispatches `ErrorBody` / `ValidationErrBody` to `Stream::Stderr`) for failures. `Stream::Stdout` / `Stream::Stderr` and the underlying `emit` function are private to `src/output.rs`; handlers never spell them. `emit_err` / `emit_response` / `emit_error` / `emit_json_error` have all been collapsed into this single surface. See [handler-shape.md](./handler-shape.md) for how `Ctx`, `Out`, and `Render` compose.

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

`Render::render_text(&self, w: &mut dyn Write)` carries the text-mode body; the JSON path goes through `serde::Serialize`. New code must not introduce `match … format`; the `format-match-dispatch` predicate fails new occurrences. See [`src/commands/codex.rs`](../../src/commands/codex.rs) for the canonical pattern.

## One emit path

Success bodies leave handlers via `ctx.out().write(&Body)?;` (or `Out::for_format(format).write(&Body)?;` for the rare `Ctx`-less verb). Failure envelopes leave handlers as `Err(Error::*)`; the dispatcher in `src/commands.rs` routes them through `output::report(format, &err)`. No handler emits its own `Stream::Stderr` envelope. If you need a bespoke failure shape, add an `Error` variant with a kebab-case discriminant; do not hand-roll a `*ErrBody` DTO. `Stream` and `emit` are private to `src/output.rs` and stay that way.

## DTOs

Response DTOs (`*Body`, `*Row`) are **top-level** structs under `mod`. Inline DTOs trip the `inline-dtos` AST predicate (DTOs declared inside *any* `Block` — function bodies, match arms, closures — count) and force per-file `#![allow(items_after_statements, …)]` waivers. The waiver is itself a refactor signal: a file that needs it is a file whose handler hasn't been migrated yet.

**Construct DTOs through `From` impls, not named builders.** Use `impl From<&Domain> for Body` so the conversion is discoverable at the trait surface and call sites read `Body::from(&domain)`. Named constructors are reserved for multi-arg or fallible builders (e.g. `RegistryProposalRow::from_kind` returns `Option<Self>`); each survivor carries a one-line doc justification.

**Typed fields, not stringly-typed ones.** `pub status` / `pub kind` (and any other field whose domain has a finite enum) carry the underlying domain enum with `#[derive(Serialize)]` + `#[serde(rename_all = "kebab-case")]`. Drop `.to_string()` at construction sites; the wire shape is unchanged.

**`PathBuf` for path fields, with `serialize_path`.** `*Body` fields that hold a filesystem path are `path: PathBuf`, serialised through `#[serde(serialize_with = "crate::output::serialize_path")]` (the helper falls back when `canonicalize` fails). Do not store `String` paths in DTOs.

**Field-type allowlist.** DTO fields use the strictest type the wire shape supports:

| Domain | Type | Notes |
|---|---|---|
| Filesystem path | `PathBuf` + `serialize_path` | never `String` |
| Status / kind / phase with finite domain | the underlying enum + `#[serde(rename_all = "kebab-case")]` | drop `.to_string()` at construction |
| Stable kebab discriminant | `&'static str` | lives in the binary |
| Timestamp written into JSON | `chrono::DateTime<Utc>` with `#[serde(with = "specify_domain::serde_rfc3339")]` | serde owns the format |
| Count | `usize` | JSON has neither `u32` nor `u64` |

**Single-variant enums are dead overhead.** Drop either the variant or the enum; the type's name already says "this DTO represents kind X". The `BriefAction::Init` pattern is the canonical example of what not to add.

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

## Errors

`specify-error::Error` variants are **structured**, not `Variant(String)` catch-alls. The kebab-case identifier in `#[error("…")]` (and in `Error::Diag.code`) is part of the public contract that skills and tests grep for; never rename without bumping `ENVELOPE_VERSION` (see [DECISIONS.md §"Wire compatibility"](../../DECISIONS.md#wire-compatibility)).

**Diag-first error policy.** New diagnostic sites use `Error::Diag { code: "<kebab>", detail: format!(…) }`. Promote to a typed `Error::*` variant **only** when:

1. A test or skill destructures the variant's payload, **or**
2. The variant routes to a non-default `Exit` slot (validation / argument / version-too-old — see [handler-shape.md §"Exit codes"](./handler-shape.md#exit-codes)), **or**
3. Three or more call sites share the variant's exact shape.

The kebab `code` is the wire contract; the Rust variant is for callers that pattern-match. Adding a typed variant for a one-site diagnostic doubles the `variant_str` table for no functional gain. When in doubt, stay on `Diag`.

A dedicated typed variant remains correct for entries that already meet the criteria above (`Error::Argument`, `Error::Validation`, `Error::PlanTransition`, `Error::ContextLockMalformed`, …).

**Hint colocation.** Long-form recovery hints live on the error, not on the renderer. `Error::hint(&self) -> Option<&'static str>` is the single hint surface; `ErrorBody::render_text` calls it. Adding a new hint means extending `Error::hint`, not the renderer.

`unwrap()` and `expect()` are reserved for invariants the type system can't express (e.g. "this enum variant covers `Status::ALL`"). Always include a justification string in `expect`. User-facing errors must surface as `Error::*` variants, not panics.

## `#[non_exhaustive]`

Every public `enum` or `struct` that may grow gets `#[non_exhaustive]`. The exception is structurally complete types (`enum Format { Json, Text }`); document the choice in a doc-line. This keeps adding a variant from being a SemVer break.

## YAML, JSON, and atomic writes

YAML (de)serialization goes through `serde-saphyr`, not `serde_yaml_ng` (retired) or `serde_yaml` (deprecated). `serde-saphyr` has no `Value` type; for dynamic YAML access deserialize into `serde_json::Value`. Deser and ser errors are wrapped behind `specify_error::YamlError` / `specify_error::YamlSerError` so the upstream crate name does not leak through every `specify-*` public surface; `specify_error::Error` carries both via `Yaml(#[from] YamlError)` and `YamlSer(#[from] YamlSerError)`, and `?` on a raw `serde_saphyr` result still propagates because `Error` also implements `From<serde_saphyr::Error>` and `From<serde_saphyr::ser::Error>` through the wrappers. Library crates use the wrapper types in their public signatures; never expose `serde_saphyr::*::Error` directly.

Writes that must not be observed mid-update use the shared atomic helpers in `specify_slice::atomic` (`yaml_write` / `bytes_write`). `fs::write` is fine for single-shot scratch files but never for files that other live processes read (`plan.yaml`, `registry.yaml`, `change.md`, `tasks.md`, `.specify/plan.lock`, `.metadata.yaml`). The `direct-fs-write` predicate fails any new `fs::write` / `std::fs::write` in non-test Rust. See [architecture.md §"Atomic writes"](./architecture.md#atomic-writes) for the rationale and [DECISIONS.md §"Atomic writes"](../../DECISIONS.md#atomic-writes) for the long form.

## Module layout

Use the modern Rust module layout: prefer `src/<parent>/<module>.rs` as the module entry point, with child modules under `src/<parent>/<module>/`. Do not add new `mod.rs` files inside module directories unless an external constraint requires it.

**Module length cap** — keep new modules ≤ 400 lines (enforced by the `module-line-count` predicate; see [predicates.md](./predicates.md)). When a file outgrows that, split by concern (one verb per file, model vs IO vs transitions, etc.) before adding more code. Prefer `src/<parent>/<module>.rs` + `src/<parent>/<module>/<concern>.rs` over a single fat file with `// ---` separators.

## No-op forwarders

A clap-parsed flag that is destructured and silently dropped (`let _ = cli.<flag>;` or pattern matches that never reach a handler) is a YAGNI smell. Either the flag is wired up (the variant carries data and the handler reads it) or it is removed from clap. The `no-op-forwarders` predicate fails new occurrences.

## Wired-but-ignored flags

A flag whose doc-comment says "Currently equivalent to the default …" or whose handler ignores the value is the same defect as `no-op-forwarders` dressed up as documentation. Drop the flag from clap until the differentiated behaviour exists.

## Drift audit

When you remove a symbol, run `rg <SymbolName> -- AGENTS.md DECISIONS.md docs/` and update every hit in the same PR. Stale symbol references in docs are worse than missing docs — they teach the reader something false. The `stale-cli-vocab` predicate catches retired CLI nouns; doc drift on internal symbols (error variants, type names, field keys) is caught only by this audit habit.
