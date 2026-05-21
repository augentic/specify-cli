//! Clap derive surface for `specify source *`. The umbrella `cli.rs`
//! re-exports `SourceAction`.

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum SourceAction {
    /// Resolve a source-adapter manifest by kebab name.
    ///
    /// Probe order: `.specify/.cache/sources/<name>/adapter.yaml`
    /// (agent-populated cache), then `<project-dir>/sources/<name>/adapter.yaml`
    /// (in-repo). Emits the resolved directory path plus the
    /// manifest's declared operations.
    Resolve {
        /// Kebab-case source-adapter name (e.g. `intent`,
        /// `documentation`, `code-typescript`, `screenshots`).
        name: String,
        /// Project directory containing `.specify/` (defaults to the
        /// current directory).
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
}
