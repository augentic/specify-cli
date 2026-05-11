# Predicates

Mechanical enforcement of the coding standards. `cargo make standards` shells out to `cargo run -p xtask -- standards-check` (followed by `--check-tightenable`). Predicates live in [`xtask/src/standards.rs`](../../xtask/src/standards.rs); per-file baselines live in [`scripts/standards-allowlist.toml`](../../scripts/standards-allowlist.toml). The xtask uses `syn` for AST predicates (so DTOs declared inside `match` arms count, where the prior regex missed them) and `regex` for textual predicates.

## The table

| Predicate | What it catches | Where it lives |
|---|---|---|
| `cli-help-shape` | Clap-derive `///` doc lines longer than 80 characters in `src/cli.rs` and `src/commands/**/cli.rs`. Help output is operator-facing and wraps poorly past 80 columns. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `crate-root-prose` | A `lib.rs` or `main.rs` whose leading `//!` doc paragraph is more than 30 non-blank lines. Long architectural prose belongs in `docs/standards/` or an in-repo RFC; the crate root should hold a 5-line summary plus a cross-link. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `direct-fs-write` | Direct `fs::write` / `std::fs::write` in non-test Rust. Managed state must use the atomic helpers (see [coding-standards.md §"YAML, JSON, and atomic writes"](./coding-standards.md#yaml-json-and-atomic-writes)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `display-serde-mirror` | `impl Display for T` where `T` derives `Serialize` and the body is `match self { Self::Variant => "literal" }` (directly, via `f.write_str`, or via `write!(f, "lit")`). The `kebab_enum!` macro replaces this pattern; a hand-rolled mirror is a regression. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `error-envelope-inlined` | `output::ErrorBody { … }` / `output::ValidationErrBody { … }` constructed outside `src/output.rs`. Error envelopes are emitted via `report`, not hand-rolled at the call site (see [handler-shape.md §"Out, Render, and emit"](./handler-shape.md#out-render-and-emit)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `format-match-dispatch` | Hand-rolled `match … format { Json => … }`. Use `Render::render_text` + `emit` instead (see [coding-standards.md §"Format dispatch"](./coding-standards.md#format-dispatch)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `inline-dtos` | Structs/enums with `#[derive(Serialize)]` declared inside any `Block` — function bodies, match arms, closures (see [coding-standards.md §"DTOs"](./coding-standards.md#dtos)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `module-line-count` | Non-test Rust source file length in lines. Default cap 400; per-file baselines grandfather oversized files until they are split (see [coding-standards.md §"Module layout"](./coding-standards.md#module-layout)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `no-op-forwarders` | `let _ = cli.<flag>;` — a parsed-but-unused CLI flag (see [coding-standards.md §"No-op forwarders"](./coding-standards.md#no-op-forwarders)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `path-helper-inlined` | `fn specify_dir|plan_path|change_brief_path|archive_dir` declared outside `crates/config/`. Path helpers live on `Layout<'a>` in `specify-config` (see [architecture.md §"Layout boundary"](./architecture.md#layout-boundary)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `result-cliresult-default` | Free `fn ... -> Result<Exit>` outside `src/commands.rs`. New handlers default to `Result<()>` and let the dispatcher collapse the success path; surviving carve-outs are grandfathered (see [handler-shape.md §"Default handler signature"](./handler-shape.md#default-handler-signature)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `rfc-numbers-in-code` | `RFC[- ]?\d+` outside `tests/`, `DECISIONS.md`, and `rfcs/` (see [coding-standards.md §"Comments"](./coding-standards.md#comments)). | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `ritual-doc-paragraphs` | The boilerplate `Returns an error if the operation fails.` doc paragraph. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `stale-cli-vocab` | Legacy CLI vocabulary in non-test Rust (`initiative`, `initiative.md`, retired top-level `specify plan`, `specify merge`, `specify validate`). Use `change`, `slice`, and the current command surface. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `unit-test-serde-roundtrip` | A `#[test]` whose body contains a matching `serde_json::*` or `serde_saphyr::*` `to_string` + `from_str` pair. Soft predicate — round-trip tests usually belong in `tests/` driven through a CLI command; allowlist when a custom Visitor or similar genuinely warrants the in-crate test. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |
| `verbose-doc-paragraphs` | A `///` doc paragraph longer than 8 consecutive non-blank lines on a `pub fn|struct|enum|const|type`. Long prose belongs in `rfcs/` or [DECISIONS.md](../../DECISIONS.md). `pub trait` is exempt. | [`xtask/src/standards.rs`](../../xtask/src/standards.rs) |

A live count strictly greater than its per-file baseline fails CI; missing predicates default to zero (new files start clean) except `module-line-count`, which defaults to 400.

## Allowlist policy

Per-file baselines live in [`scripts/standards-allowlist.toml`](../../scripts/standards-allowlist.toml). Each entry grandfathers an existing file at its current count for one predicate; the file is expected to ratchet down over time, never up.

**Ratchet** — any PR that touches a file with allowlist baselines is expected to reduce them where it can. CI runs `cargo run -p xtask -- standards-check --check-tightenable`, which fails when an unrelated PR could lower a baseline without code changes. Run `cargo make xtask standards-check --tighten` and commit the updated `scripts/standards-allowlist.toml` to clear.

New violations are never allowlisted to silence them — fix the code or split the file. The allowlist is only for the migration tail of files that pre-date a predicate.

## Drift audit (manual)

The `stale-cli-vocab` predicate catches retired CLI nouns in code; documentation drift on internal symbols (error variants, type names, field keys) is not mechanically enforced. When you remove a symbol, run `rg <SymbolName> -- AGENTS.md DECISIONS.md docs/` and update every hit in the same PR. See [coding-standards.md §"Drift audit"](./coding-standards.md#drift-audit).
