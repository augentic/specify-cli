//! `specdev` clap surface.

use clap::{Parser, Subcommand};

use crate::output::Format;

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

    /// Output format. `text` (the default) emits the human-oriented
    /// stdout / stderr summary unchanged. `json` emits an RFC-28
    /// §"Review result envelope" body
    /// (`{version: 1, summary, findings: [LintFinding]}`) to stdout
    /// on both success and failure; the exit code is `2` when any
    /// findings are present (existing validation semantics) and `1`
    /// when an infrastructure error prevents the checks from running,
    /// in which case the envelope on stdout collapses to
    /// `{version: 1, summary: {all zero}, findings: []}` and the
    /// underlying error surfaces on stderr. Set `SPECDEV_FORMAT=json`
    /// to default to the structured envelope.
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
