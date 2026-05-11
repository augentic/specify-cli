//! Clap derive surface for `specify registry *`.
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports `RegistryAction` so
//! the parent derives still resolve at expansion time.

use clap::Subcommand;

/// Registry operations on `registry.yaml`.
#[derive(Subcommand)]
pub(crate) enum RegistryAction {
    /// Print the parsed `registry.yaml` (text or JSON). Absent file exits 0.
    Show,
    /// Validate `registry.yaml` shape. Absent file exits 0.
    Validate,
    /// Append a new project entry to `registry.yaml`. Creates the file
    /// when absent.
    Add {
        /// Kebab-case project name. Must be unique within the registry.
        name: String,
        /// Clone target — `.`, a repo-relative path, `git@host:path`, or
        /// `http(s)://` / `ssh://` / `git+...` remote.
        #[arg(long)]
        url: String,
        /// Capability identifier (e.g. `omnia@v1`). Non-empty after trim.
        #[arg(long)]
        capability: String,
        /// Domain-level characterisation; required when the registry
        /// declares more than one project.
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove an existing project entry. Warns when `plan.yaml` references it.
    Remove {
        /// Kebab-case project name to remove.
        name: String,
    },
}
