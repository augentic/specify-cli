//! Clap derive surface for `specify compatibility *`. The umbrella
//! `cli.rs` re-exports `CompatibilityAction`.

use clap::Subcommand;

/// Contract compatibility classification verbs.
#[derive(Subcommand)]
pub(crate) enum CompatibilityAction {
    /// Check current producer/consumer contract compatibility.
    Check,
    /// Render a classified compatibility report.
    Report {
        /// Kebab-case change name to echo in the report.
        #[arg(long)]
        change: String,
    },
}
