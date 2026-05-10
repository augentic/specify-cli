//! Clap derive surface for `specify change *` (the umbrella verb).
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports `ChangeAction` so
//! the parent derives still resolve at expansion time. The nested
//! `plan *` and `plan lock *` enums live next to their dispatchers in
//! [`crate::commands::change::plan::cli`].

use clap::Subcommand;

use crate::commands::change::plan::cli::PlanAction;

/// Umbrella `change` verbs — owns `change.md` and `plan.yaml`.
#[derive(Subcommand)]
pub enum ChangeAction {
    /// Scaffold `change.md` at the repo root. Refuses to overwrite.
    Create {
        /// Kebab-case change name (baked into the frontmatter).
        name: String,
    },
    /// Print the parsed change brief (text or JSON). Absent file exits 0.
    Show,
    /// Manage the change's executable plan (`plan.yaml`).
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Close out a change once every plan entry is terminal and every
    /// per-project PR has been operator-merged on its remote. Atomic:
    /// any guard failure leaves on-disk state untouched. Never merges
    /// PRs — operator lands them first through the forge.
    Finalize {
        /// Remove `.specify/workspace/<peer>/` clones after archiving.
        /// Refused when any clone has a dirty working tree.
        #[arg(long)]
        clean: bool,
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}
