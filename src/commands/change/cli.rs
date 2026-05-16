//! Clap derive surface for `specify change *` — the operator-facing
//! Layer 1 verbs that own `change.md` and `plan.yaml`.
//!
//! The executable plan moved to its own top-level verb after the
//! `change plan *` flatten — see [`crate::commands::plan::cli`]. The
//! remaining verbs here are peer Layer 1 commands supporting peer
//! Layer 2 skills (`draft`, `show`, `finalize`, `survey`).

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::SourceArg;

/// `change` verbs — own `change.md` and `plan.yaml`.
#[derive(Subcommand)]
pub enum ChangeAction {
    /// Scaffold `change.md` and `plan.yaml` at the repo root in one
    /// shot. Atomic: refuses if either file already exists, and writes
    /// neither file in that case. Delegates the plan half to the same
    /// helper that backs `specify plan create`.
    Draft {
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
    /// Mechanically scan legacy sources for externally observable
    /// surfaces, write `surfaces.json` + `metadata.json` per source.
    Survey {
        /// Single-source mode: path to the legacy source root.
        #[arg(conflicts_with = "sources")]
        source_path: Option<PathBuf>,

        /// Single-source mode: kebab-case source key.
        #[arg(long, requires = "source_path")]
        source_key: Option<String>,

        /// Batch mode: YAML file listing one row per source.
        #[arg(long, conflicts_with_all = ["source_path", "source_key"])]
        sources: Option<PathBuf>,

        /// Output directory for `surfaces.json` and `metadata.json`.
        #[arg(long)]
        out: PathBuf,
    },
}
