//! Clap derive surface for `specify capability *`.
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports `CapabilityAction`
//! so the parent derives still resolve at expansion time.

use std::path::PathBuf;

use clap::Subcommand;
use specify_domain::capability::Phase;

#[derive(Subcommand)]
pub(crate) enum CapabilityAction {
    /// Resolve a capability value to a directory path
    Resolve {
        /// Capability value (bare name or URL) to resolve through the
        /// project-local cache and bundled capability lookup
        capability_value: String,
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
    /// Validate a `capability.yaml` file.
    Check {
        /// Directory containing `capability.yaml`
        capability_dir: PathBuf,
    },
    /// List the briefs for a phase in topological order (optionally
    /// with completion status against a specific slice)
    Pipeline {
        /// Pipeline phase to enumerate
        #[arg(value_enum)]
        phase: Phase,
        /// Slice directory; when supplied, each brief includes a
        /// `present` boolean reflecting whether its `generates`
        /// artifact exists under the directory
        #[arg(long)]
        slice: Option<PathBuf>,
    },
}
