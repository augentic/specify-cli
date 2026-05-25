//! Clap derive surface for `specrun tool *`. The umbrella `cli.rs`
//! re-exports `ToolAction`.

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
    /// Fetch one declared tool, or every declared tool when omitted.
    Fetch {
        /// Optional declared tool name to fetch.
        name: Option<String>,
    },
    /// Remove unused cache entries for the current project.
    Gc,
}
