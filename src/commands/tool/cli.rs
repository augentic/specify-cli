//! Clap derive surface for `specify tool *`.
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports `ToolAction` so the
//! parent derives still resolve at expansion time.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum ToolAction {
    /// Fetch if needed, then run a declared WASI tool.
    Run {
        /// Declared tool name.
        name: String,
        /// Args forwarded to the tool after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// List declared tools and cache status.
    List,
    /// Fetch one declared tool, or every declared tool when omitted.
    Fetch {
        /// Optional declared tool name to fetch.
        name: Option<String>,
    },
    /// Show one declared tool's metadata.
    Show {
        /// Declared tool name.
        name: String,
    },
    /// Remove unused cache entries for the current project.
    Gc,
}
