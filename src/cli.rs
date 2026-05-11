//! Top-level clap derive surface for the `specify` binary.
//!
//! This module owns only the umbrella types: [`Cli`], [`Commands`],
//! [`OutputFormat`], and the [`SourceArg`] `--source key=value` parser.
//! Per-verb action enums live next to their dispatchers in
//! `src/commands/<verb>/cli.rs` and are re-exported below so the clap
//! derive on [`Commands`] resolves them at expansion time.

use std::str::FromStr;

use clap::{Parser, Subcommand, ValueEnum};

pub use crate::commands::capability::cli::CapabilityAction;
pub use crate::commands::change::cli::ChangeAction;
pub use crate::commands::change::plan::cli::{LockAction, PlanAction};
pub use crate::commands::codex::cli::CodexAction;
pub use crate::commands::compatibility::cli::CompatibilityAction;
pub use crate::commands::context::cli::ContextAction;
pub use crate::commands::registry::cli::RegistryAction;
pub use crate::commands::slice::cli::{
    JournalAction, OutcomeAction, OutcomeKindAction, RegistryAmendmentArgs, SliceAction,
    SliceMergeAction, SliceTaskAction,
};
pub use crate::commands::tool::cli::ToolAction;
pub use crate::commands::workspace::cli::WorkspaceAction;

#[derive(Parser)]
#[command(
    name = "specify",
    version,
    about = "Specify CLI — deterministic operations for spec-driven development"
)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Output format. `text` by default; pass `--format json` (or set
    /// `SPECIFY_FORMAT=json`) for structured envelopes when shelling
    /// out from skills.
    #[arg(long, env = "SPECIFY_FORMAT", default_value = "text", global = true)]
    pub(crate) format: OutputFormat,
}

#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize .specify/ in a project.
    ///
    /// Pass `<capability>` (bare name or URL) for a regular project, or
    /// `--hub` for a registry-only platform hub. The two are mutually
    /// exclusive.
    Init {
        /// Capability identifier or URL (e.g. `omnia`,
        /// `https://github.com/<owner>/<repo>/capabilities/<name>`).
        /// Required unless `--hub` is set.
        capability: Option<String>,
        /// Project name (defaults to the project directory name)
        #[arg(long)]
        name: Option<String>,
        /// Project domain description (tech stack, architecture, testing)
        #[arg(long)]
        domain: Option<String>,
        /// Scaffold a registry-only platform hub instead of a regular
        /// project. Refuses to run when `.specify/` already exists.
        #[arg(long)]
        hub: bool,
    },

    /// Project dashboard — registry summary, plan progress, active changes
    Status,

    /// Refresh AGENTS.md and check whether generated context is current.
    Context {
        #[command(subcommand)]
        action: ContextAction,
    },

    /// Capability operations
    Capability {
        #[command(subcommand)]
        action: CapabilityAction,
    },

    /// Codex rule catalogue operations
    Codex {
        #[command(subcommand)]
        action: CodexAction,
    },

    /// WASI tool runner.
    Tool {
        #[command(subcommand)]
        action: ToolAction,
    },

    /// Cross-project contract compatibility reports.
    Compatibility {
        #[command(subcommand)]
        action: CompatibilityAction,
    },

    /// Slice lifecycle operations — one `define → build → merge` loop.
    Slice {
        #[command(subcommand)]
        action: SliceAction,
    },

    /// Change orchestration — operator brief, plan, finalize.
    Change {
        #[command(subcommand)]
        action: ChangeAction,
    },

    /// Platform registry at `registry.yaml` (repo root)
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// Materialise and manage registry peers under `.specify/workspace/`.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Generate shell completions for the given shell.
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Typed `--source <key>=<path-or-url>` CLI value.
///
/// The [`FromStr`] impl returns a `String` error on malformed input so
/// clap surfaces a standard usage diagnostic (exit code 2). Call sites
/// read `arg.key` / `arg.value` instead of unpacking a positional tuple.
#[derive(Debug, Clone)]
pub struct SourceArg {
    /// Source key (left of `=`).
    pub key: String,
    /// Source value — path or URL (right of `=`).
    pub value: String,
}

impl FromStr for SourceArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (k, v) = s
            .split_once('=')
            .ok_or_else(|| format!("--source must be <key>=<path-or-url>, got `{s}`"))?;
        if k.is_empty() || v.is_empty() {
            return Err(format!("--source key and value must be non-empty, got `{s}`"));
        }
        Ok(Self {
            key: k.to_string(),
            value: v.to_string(),
        })
    }
}
