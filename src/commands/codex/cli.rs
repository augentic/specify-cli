//! Clap derive surface for `specify codex *`. The umbrella `cli.rs`
//! re-exports `CodexAction`.

use clap::Subcommand;

/// Project-resolved codex rule catalogue verbs.
#[derive(Subcommand, Copy, Clone)]
pub enum CodexAction {
    /// Export the resolved codex as JSON.
    Export,
}
