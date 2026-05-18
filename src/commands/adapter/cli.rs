//! Clap derive surface for `specify capability *`. The umbrella
//! `cli.rs` re-exports `CapabilityAction`.

use std::path::PathBuf;

use clap::Subcommand;
use specify_domain::capability::Phase;

#[derive(Subcommand)]
pub enum CapabilityAction {
    /// Resolve a capability value to a directory path
    Resolve {
        /// Capability value (bare name or URL) to resolve through the
        /// project-local cache and bundled capability lookup
        capability_value: String,
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
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
