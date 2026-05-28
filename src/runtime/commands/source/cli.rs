//! Clap derive surface for `specrun source *`. The umbrella `cli.rs`
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
    /// `--explain` switches the output to the extraction cache fingerprint contract fingerprint
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

    /// Run a source adapter's enumerate + extract in isolation
    /// (`specrun source preview` contract).
    ///
    /// Resolves the adapter manifest, validates the `--source` path,
    /// scaffolds the output directory with an `evidence/` subtree, and
    /// emits a summary of adapter info and brief paths. The agent then
    /// executes the briefs against the prepared environment.
    ///
    /// Workflow-free: nothing is written into `.specify/`, no lifecycle
    /// moves, and no journal events fire. Output lives entirely under
    /// `--out`.
    Preview {
        /// Kebab-case source-adapter name (e.g. `screenshots`,
        /// `code-typescript`, `documentation`).
        adapter: String,
        /// Bound source path (`$SOURCE_DIR` for the adapter's briefs).
        #[arg(long)]
        source: PathBuf,
        /// Restrict extraction to specific candidate IDs; defaults to
        /// all candidates discovered by `enumerate`.
        #[arg(long)]
        candidate: Vec<String>,
        /// Output directory for Evidence files (default:
        /// `.specify-preview/`). Created if absent.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Project directory used for adapter resolution (defaults to
        /// the current directory). Does not require an initialised
        /// `.specify/` directory.
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
}
