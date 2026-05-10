//! Clap derive surface for `specify compatibility *`.
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports `CompatibilityAction`
//! so the parent derives still resolve at expansion time.

use clap::Subcommand;

/// Contract compatibility classification verbs.
#[derive(Subcommand)]
pub enum CompatibilityAction {
    /// Check current producer/consumer contract compatibility.
    Check,
    /// Render a classified compatibility report.
    Report {
        /// Kebab-case change name to echo in the report.
        #[arg(long)]
        change: String,
    },
}
