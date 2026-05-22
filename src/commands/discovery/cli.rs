//! Clap derive surface for `specify discovery *`. The umbrella
//! `cli.rs` re-exports `DiscoveryAction`.
//!
//! RFC-27 §D6 — `specify discovery show --aliases` is the read-only
//! window into the candidate inventory's alias map. The verb owns
//! inspection only; writes belong to `/spec:plan` (initial enumerate)
//! and `specify plan amend --add-alias` / `--remove-alias` (operator
//! edits).

use clap::Subcommand;

#[derive(Subcommand)]
pub enum DiscoveryAction {
    /// Render the candidate inventory from `<project_dir>/discovery.md`.
    ///
    /// Default output mirrors the on-disk block grammar: one section
    /// per candidate with the `id`, `sources`, and `summary` bullets.
    /// `--aliases` switches to the RFC-27 §D6 alias map view —
    /// `<id> -> [<alias>, <alias>, …]` lines sorted by `id`, omitting
    /// candidates with no aliases.
    Show {
        /// Render the alias map instead of the default candidate
        /// inventory. Sort order matches `candidate.id`.
        #[arg(long)]
        aliases: bool,
    },
}
