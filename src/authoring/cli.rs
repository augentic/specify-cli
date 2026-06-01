//! `specdev` clap surface.

use clap::{Parser, Subcommand};

use crate::authoring::commands::lint::LintAction;
use crate::output::Format;

#[derive(Debug, Parser)]
#[command(
    name = "specdev",
    about = "Framework authoring lint for augentic/specify",
    version,
    after_help = "Common entry points:\n  specdev lint --framework-root .\n  make check"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Failure-envelope shape on infrastructure error. `text` (the
    /// default) emits the human-oriented `error: ...` line on
    /// stderr; `json` emits the empty `DiagnosticReport` envelope on
    /// stdout alongside the stderr error message so structured
    /// consumers can rely on a stable wire shape. The per-subcommand
    /// `lint --output-format` flag controls the success-body format
    /// (`{ json, pretty, github, compact }`); when unset, the
    /// success body inherits this flag (`json` → `Json`, `text` →
    /// `Pretty`).
    #[arg(long, env = "SPECDEV_FORMAT", default_value = "text", global = true)]
    pub format: Format,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Lint the framework repo — combine the imperative `Check`
    /// predicates with the declarative deterministic-hint
    /// interpreter and emit one structured envelope per run.
    Lint(LintAction),
}
