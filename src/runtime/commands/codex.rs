//! `specrun codex {export}` — read-only codex resolution surface.
//!
//! The export verb is the RFC-28 §"Resolved codex export" CLI entry
//! point. It does not require an initialised `.specify/` tree:
//! callers supply `--codex-root` directly (or rely on the resolver's
//! probe step 2 against a monorepo `{project_dir}/adapters/shared/
//! codex/universal/`). No journal events, no lifecycle transitions,
//! no on-disk writes — the handler streams a `ResolvedCodex` JSON
//! envelope to stdout and exits.

pub mod cli;
pub mod export;
