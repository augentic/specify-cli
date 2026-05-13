//! Clap derive surface for `specify context *`. The umbrella `cli.rs`
//! re-exports `ContextAction`.

use clap::Subcommand;

/// Refreshable repository context for agent-facing guidance.
#[derive(Subcommand)]
pub enum ContextAction {
    /// Generate or refresh the managed `AGENTS.md` context block.
    Generate {
        /// Exit non-zero if AGENTS.md or the context lock would change; do not write.
        #[arg(long)]
        check: bool,
        /// Rewrite managed context despite unfenced or edited generated content.
        #[arg(long)]
        force: bool,
    },
    /// Check whether `AGENTS.md` matches current repository inputs.
    Check,
}
