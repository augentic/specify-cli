# Predicates

Mechanical enforcement of the coding standards. `cargo make standards` shells out to `cargo run -p xtask -- standards-check`. Predicates live in [`xtask/src/standards.rs`](../../xtask/src/standards.rs). The xtask uses `syn` for AST predicates and `regex` for textual predicates.

## The table

| Predicate | What it catches |
|---|---|
| `cli-help-shape` | Clap-derive `///` doc lines longer than 80 characters in `src/cli.rs` and `src/commands/**/cli.rs`. Help output is operator-facing and wraps poorly past 80 columns. |
| `crate-root-prose` | A `lib.rs` or `main.rs` whose leading `//!` doc paragraph is more than 30 non-blank lines. Long architectural prose belongs in [`docs/standards/`](./) or an in-repo RFC; the crate root should hold a 5-line summary plus a cross-link (see [style.md §"No archaeology in code"](./style.md#no-archaeology-in-code)). |
| `direct-fs-write` | Direct `fs::write` / `std::fs::write` in non-test Rust. Managed state must use the atomic helpers (see [coding-standards.md §"YAML, JSON, and atomic writes"](./coding-standards.md#yaml-json-and-atomic-writes)). |
| `error-envelope-inlined` | `output::ErrorBody { … }` / `output::ValidationErrBody { … }` constructed outside `src/output.rs`. Error envelopes are emitted via `report`, not hand-rolled at the call site (see [handler-shape.md §"Out, Render, and emit"](./handler-shape.md#out-render-and-emit)). |
| `mod-rs-forbidden` | Any source file named `mod.rs` under `src/` or `crates/`. The 2018-edition `<module>/mod.rs` layout is retired in favour of `<module>.rs` + `<module>/<concern>.rs`; the walker exempts `tests/`, where `tests/<helper>/mod.rs` is the sanctioned cargo idiom (see [coding-standards.md §"Module layout"](./coding-standards.md#module-layout)). |
| `path-helper-inlined` | `fn specify_dir|plan_path|change_brief_path|archive_dir` declared outside `crates/config/`. Path helpers live on `Layout<'a>` in `specify-config` (see [architecture.md §"Layout boundary"](./architecture.md#layout-boundary)). |
| `result-cliresult-default` | Free `fn ... -> Result<Exit>` outside `src/commands.rs`. New handlers default to `Result<()>` and let the dispatcher collapse the success path (see [handler-shape.md §"Default handler signature"](./handler-shape.md#default-handler-signature)). |
| `stale-cli-vocab` | Legacy CLI vocabulary in non-test Rust (`initiative`, `initiative.md`, retired top-level `specify plan`, `specify merge`, `specify validate`). Use `change`, `slice`, and the current command surface. |
| `verbose-doc-paragraphs` | A `///` doc paragraph longer than 8 consecutive non-blank lines on a `pub fn|struct|enum|const|type`. Long prose belongs in `rfcs/` or [DECISIONS.md](../../DECISIONS.md). `pub trait` is exempt. |

A live count strictly greater than zero fails CI for any of these predicates — they are all zero-baseline. Fix the code or split the file.

## Drift audit (manual)

The `stale-cli-vocab` predicate catches retired CLI nouns in code; documentation drift on internal symbols (error variants, type names, field keys) is not mechanically enforced. When you remove a symbol, run `rg <SymbolName> -- AGENTS.md DECISIONS.md docs/` and update every hit in the same PR. See [coding-standards.md §"Drift audit"](./coding-standards.md#drift-audit).
