//! Clap derive surface for `specrun registry *`. The umbrella
//! `cli.rs` re-exports `RegistryAction`.

use clap::Subcommand;

/// Registry operations on `registry.yaml`.
#[derive(Subcommand)]
pub enum RegistryAction {
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
        /// Adapter identifier (e.g. `omnia@v1`). Non-empty after trim.
        #[arg(long)]
        adapter: String,
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
