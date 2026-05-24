//! Clap derive surface for `specify source *`. The umbrella `cli.rs`
//! re-exports `SourceAction`.

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum SourceAction {
    /// Resolve a source-adapter manifest by kebab name.
    ///
    /// Probe order: `.specify/.cache/manifests/sources/<name>/adapter.yaml`
    /// (agent-populated manifest cache), then
    /// `<project-dir>/adapters/sources/<name>/adapter.yaml`
    /// (in-repo). Emits the resolved directory path plus the
    /// manifest's declared operations.
    ///
    /// `--explain` switches the output to the workflow §D8 fingerprint
    /// chain read from `.specify/.cache/extractions/<name>/index.jsonl`
    /// instead of the manifest summary.
    Resolve {
        /// Kebab-case source-adapter name (e.g. `intent`,
        /// `documentation`, `code-typescript`, `screenshots`).
        name: String,
        /// Project directory containing `.specify/` (defaults to the
        /// current directory).
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
        /// Print the fingerprint chain from
        /// `.specify/.cache/extractions/<name>/index.jsonl` instead of the
        /// manifest summary.
        #[arg(long)]
        explain: bool,
    },
}
