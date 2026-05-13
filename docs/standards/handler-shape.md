# Handler shape

The contract every CLI command handler obeys: how `Ctx` is constructed, how output flows through `ctx.write` / `output::write` / `emit`, which exit code a terminal `Error` maps to, and what the dispatcher between `clap` and the workspace crates is allowed to do.

## Ctx construction

Handlers take `&Ctx` (renamed from `CommandContext` so the module path `crate::context::Ctx` carries the noun). `Ctx` exposes the resolved project dir, layout, output format, and a few thin facade methods for handler ergonomics; everything else flows through workspace crates. `Layout<'a>` lives on `Ctx` rather than at call sites so path helpers stay anchored in `specify-domain` — see [architecture.md §"Layout boundary"](./architecture.md#layout-boundary).

## Default handler signature

Command handlers default to `Result<()>` (success-path conversion happens at the dispatcher boundary). Surface non-success exits through typed errors that `Exit::from(&Error)` maps to the four-slot exit table — do **not** return `Result<Exit>` to thread a non-zero code by hand.

```rust
// GOOD — default shape
pub(crate) fn handle(ctx: &Ctx, args: &SomeArgs) -> Result<()> {
    let body = some_crate::do_work(ctx.layout(), args)?;
    ctx.write(&SomeBody::from(&body), write_text)?;
    Ok(())
}

// GOOD — explicit Result<Exit> only when the handler needs a
// non-success exit and a typed *ErrBody (rare — workspace::push is one).
pub(crate) fn handle(ctx: &Ctx) -> Result<Exit, Error> { /* ... */ }
```

A free `fn ... -> Result<Exit>` belongs in `src/commands.rs`. Elsewhere, default to `Result<()>` and let the dispatcher collapse the success path.

## ctx.write, output::write, and emit

Success bodies leave handlers via `ctx.write(&body, write_text)?;`. `Ctx::write` chooses the JSON vs text path based on `Format`; the handler never sees the branch. The `write_text` closure has signature `FnOnce(&mut dyn Write, &T) -> std::io::Result<()>` and is colocated with each handler so the response shape stays in a single block of code; the JSON path goes through `serde::Serialize` automatically.

`Stream::Stdout` / `Stream::Stderr` and the underlying `emit` function are private to `src/output.rs`. Handlers never spell them. Format-only handlers that run before (or outside of) a `Ctx` — `commands::init::run`, `commands::capability::resolve`, `commands::capability::check` — receive a bare `Format` and call the free `output::write(format, &body, write_text)?;` instead.

For the full DTO and dispatch rules see [coding-standards.md §"Format dispatch"](./coding-standards.md#format-dispatch), [§"One emit path"](./coding-standards.md#one-emit-path), and [§"DTOs"](./coding-standards.md#dtos). The canonical pattern is [`src/commands/codex.rs`](../../src/commands/codex.rs).

## Exit codes

The four-slot CLI exit-code table is fixed:

| Code | Name | When |
|---|---|---|
| 0 | `EXIT_SUCCESS` | Command succeeded |
| 1 | `EXIT_GENERIC_FAILURE` | Default `Error` → exit 1 |
| 2 | `EXIT_VALIDATION_FAILED` | `Error::Validation`, undeclared/over-permissioned tool, `Error::Argument` |
| 3 | `EXIT_VERSION_TOO_OLD` | `Error::CliTooOld` (`specify-version-too-old` in JSON) |

`Exit::from(&Error)` in [`src/output.rs`](../../src/output.rs) is the single source of truth. Every dispatcher in `src/commands/*` routes its terminal error through `report`, which calls `Exit::from`. Do not invent new exit codes. The long-form decision (including `Exit::Code(u8)`'s WASI passthrough role) lives in [DECISIONS.md §"Exit codes"](../../DECISIONS.md#exit-codes).

## Dispatcher contract

`src/cli.rs` declares the clap derive surface. Every command has a doc comment that doubles as `--help` output — keep it accurate and operator-facing (no internal jargon, no RFC numbers without a hyperlink). Add new commands as enum variants on `Commands` with a nested action enum where the verb has subactions; mirror existing groups (`SliceAction`, `ChangeAction`, etc.).

`--source key=value` arguments are parsed via the typed `SourceArg` (`impl FromStr for SourceArg`) so call sites read named fields instead of tuple positions.

Dispatchers live in `src/commands/<verb>.rs` and call back into the workspace crates. The discipline is:

1. Clap parses argv → `Commands` enum.
2. `src/commands.rs` matches the variant and calls the dispatcher in `src/commands/<verb>.rs`.
3. The dispatcher loads `ProjectConfig` (which enforces the `specify_version` floor for free) and any other state it needs.
4. The dispatcher delegates the deterministic work to a workspace crate (`specify_slice`, `specify_change`, etc.) and converts the result to a `*Body` for `ctx.write(&body, write_text)`.

Failure envelopes leave handlers as `Err(Error::*)`; the dispatcher in `src/commands.rs` routes them through `output::report(format, &err)`. No handler emits its own `Stream::Stderr` envelope.

Never put domain logic in the binary. If a function needs unit tests, it belongs in a workspace crate. The binary owns argv parsing, formatting, and dispatch only. For the crate dependency direction this enforces see [architecture.md §"Workspace layout"](./architecture.md#workspace-layout).

## Gotcha — `specify init` and the version floor

`specify init` bypasses the `specify_version` floor check (the file doesn't exist yet); every other project-aware verb inherits it for free via `ProjectConfig::load`. Don't reimplement the floor check at a subcommand site.
