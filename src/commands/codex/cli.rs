//! Clap derive surface for `specify codex *`.
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports `CodexAction` so the
//! parent derives still resolve at expansion time.

use clap::Subcommand;

/// Project-resolved codex rule catalogue verbs.
#[derive(Subcommand)]
pub enum CodexAction {
    /// List resolved codex rules.
    List,
    /// Show one resolved codex rule.
    Show {
        /// Stable codex rule id, e.g. `UNI-002`.
        rule_id: String,
    },
    /// Validate the resolved codex rule set.
    Validate,
    /// Export the resolved codex as JSON.
    Export,
}
