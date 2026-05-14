//! Clap derive surface for `specify compatibility *`. The umbrella
//! `cli.rs` re-exports `CompatibilityAction`.

use clap::Subcommand;

/// Contract compatibility classification verbs.
#[derive(Subcommand)]
pub enum CompatibilityAction {
    /// Classify cross-project producer/consumer contract deltas.
    ///
    /// Without flags, exits validation-failed (`2`) when any finding is
    /// `breaking`, `ambiguous`, or `unverifiable`. Pass `--change` to
    /// echo a kebab-case change name in the report. Pass
    /// `--report-only` to render the same payload without the strict
    /// exit-code semantics — the read-only RM-04 report previously
    /// surfaced through `specify compatibility report`.
    Check {
        /// Kebab-case change name to echo in the report.
        #[arg(long)]
        change: Option<String>,
        /// Render the classified report without the validation-failed
        /// exit code; matches the pre-merge `compatibility report
        /// --change <name>` shape.
        #[arg(long = "report-only")]
        report_only: bool,
    },
}
