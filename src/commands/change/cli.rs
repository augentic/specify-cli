//! Clap derive surface for `specify change *` (the operator umbrella).
//!
//! The executable plan moved to its own top-level verb after the
//! `change plan *` flatten — see [`crate::commands::plan::cli`]. The
//! umbrella retains only the operator-facing brief verbs (`create`,
//! `show`, `finalize`).

use clap::Subcommand;

use crate::cli::SourceArg;

/// Umbrella `change` verbs — owns `change.md` and `plan.yaml`.
#[derive(Subcommand)]
pub enum ChangeAction {
    /// Scaffold `change.md` and `plan.yaml` at the repo root in one
    /// shot. Atomic: refuses if either file already exists, and writes
    /// neither file in that case. Delegates the plan half to the same
    /// helper that backs `specify plan create`.
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
