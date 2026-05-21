//! Clap derive surface for `specify target *`. The umbrella `cli.rs`
//! re-exports `TargetAction`.

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum TargetAction {
    /// Resolve a target-adapter manifest by kebab name (or
    /// `name@version` value).
    ///
    /// Probe order: `.specify/.cache/targets/<name>/adapter.yaml`
    /// (agent-populated cache), then `<project-dir>/targets/<name>/adapter.yaml`
    /// (in-repo). Emits the resolved directory path plus the
    /// manifest's declared operations.
    Resolve {
        /// Target-adapter identifier — kebab name or `name@version`
        /// (e.g. `omnia`, `vectis`, `contracts@v1`). The optional
        /// `@version` suffix is treated as an opaque identifier and
        /// is stripped for the manifest lookup.
        value: String,
        /// Project directory containing `.specify/` (defaults to the
        /// current directory).
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
}
