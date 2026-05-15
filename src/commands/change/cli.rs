//! Clap derive surface for `specify change *` (the umbrella verb).
//! The nested `plan *` and `plan lock *` enums live next to their
//! dispatchers in [`crate::commands::change::plan::cli`].

use clap::Subcommand;

use crate::cli::SourceArg;
use crate::commands::change::plan::cli::PlanAction;

/// Umbrella `change` verbs — owns `change.md` and `plan.yaml`.
#[derive(Subcommand)]
pub enum ChangeAction {
    /// Scaffold `change.md` and `plan.yaml` at the repo root in one
    /// shot. Atomic: refuses if either file already exists, and writes
    /// neither file in that case.
    Create {
        /// Kebab-case change name (baked into both the brief
        /// frontmatter and the plan).
        name: String,
        /// Named source, repeated: --source `<key>`=`<path-or-url>`.
        /// Recorded in the plan's `sources:` map.
        #[arg(long = "source")]
        sources: Vec<SourceArg>,
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
