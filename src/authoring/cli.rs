//! `specdev` clap surface.

use clap::{Parser, Subcommand};

use crate::shared::format::Format;

#[derive(Debug, Parser)]
#[command(
    name = "specdev",
    about = "Framework authoring checks for augentic/specify",
    version,
    after_help = "Common entry points:\n  specdev check --framework-root .\n  make check"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Output format. `text` by default; pass `--format json` (or set
    /// `SPECDEV_FORMAT=json`) for structured validation summaries.
    #[arg(long, env = "SPECDEV_FORMAT", default_value = "text", global = true)]
    pub format: Format,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run framework consistency checks over a framework repo root.
    Check {
        /// Path to the augentic/specify framework repository.
        #[arg(long, env = "SPECDEV_FRAMEWORK_ROOT")]
        framework_root: std::path::PathBuf,
    },
}
