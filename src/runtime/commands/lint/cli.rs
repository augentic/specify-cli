//! Clap derive surface for `specrun lint *`. Mirrors the
//! `RulesAction` shape in `src/runtime/commands/rules/cli.rs`.
//!
//! The per-subcommand `--format` flag is intentionally distinct from
//! the global `Cli::format` flag: global `--format` toggles JSON vs
//! text for envelope-emitting handlers and the failure path, while
//! `specrun lint --format` selects the closed the diagnostics formatter set set
//! (`{ json, pretty, github, compact }`). The handler reads its own
//! per-subcommand flag and ignores the global one for the success
//! body.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use specify_lints::lint::diagnostics::Format as DiagnosticsFormat;

#[derive(Subcommand)]
pub enum LintAction {
    /// Resolve applicable rules, build a `WorkspaceModel`,
    /// evaluate deterministic hints, and emit the structured review
    /// envelope.
    Run(RunArgs),
}

/// Flag surface for `specrun lint run`. Grouped into one struct so the
/// handler threads a single reference instead of a positional argument
/// list.
#[derive(Args)]
pub struct RunArgs {
    /// Codex root supplying shared `UNI-*` rules. Resolution
    /// order (rules-root resolution): this flag → `$RULES_ROOT` env →
    /// project's `.specify/cache/rules/` → bundled tree.
    /// Validation failure exits 2 with `rules-root-required`.
    #[arg(long, env = "RULES_ROOT")]
    pub rules_root: Option<PathBuf>,

    /// Target-adapter name (kebab, optionally `<name>@v<major>`).
    #[arg(long)]
    pub target: String,

    /// Source-adapter name; repeatable. Each occurrence
    /// contributes one source overlay to the resolved codex.
    #[arg(long = "source", value_name = "NAME")]
    pub sources: Vec<String>,

    /// Restrict the scan to one slice's tasks (lint scope resolution).
    /// Reads the slice's `tasks.md` for `Touches:` / `Produces:`
    /// paths plus `.specify/slices/<name>/**`.
    #[arg(long)]
    pub slice: Option<String>,

    /// Restrict the scan to specific artifact paths
    /// (lint scope resolution). Repeatable; composes with `--slice` (union).
    #[arg(long = "artifact", value_name = "PATH")]
    pub artifacts: Vec<PathBuf>,

    /// Lowercase language token; repeatable. Passed to both
    /// `specrun rules export` and the consumer indexer.
    #[arg(long = "language", value_name = "TOKEN")]
    pub languages: Vec<String>,

    /// Emit the `WorkspaceModel` only (debug). Validates the
    /// model against `WORKSPACE_MODEL_JSON_SCHEMA` before
    /// stdout emit; skips hint evaluation entirely.
    #[arg(long)]
    pub dump_model: bool,

    /// Upgrade the reserved-hint diagnostics reserved-hint summary finding's
    /// severity from `optional` to `important`, which
    /// contributes to a non-zero exit code per lint exit mapping.
    #[arg(long)]
    pub strict_hints: bool,

    /// Include `CORE-*` rules resolved from
    /// `adapters/shared/rules/core/` (RFC-34 §A3 / §F3).
    /// Default off — consumer-project review runs never evaluate
    /// `CORE-*` hints unless the caller opts in.
    #[arg(long)]
    pub include_core: bool,

    /// Output format. Closed Phase 2 set per the diagnostics formatter set:
    /// `{ json, pretty, github, compact }`; default `pretty`.
    ///
    /// Spelled `--output-format` rather than `--format` to
    /// avoid a clap conflict with the global `--format` flag
    /// on the `Cli` (text vs JSON for the failure envelope).
    /// clap's `debug_asserts` refuses two flags whose long
    /// name OR derived id agree, so the per-subcommand axis
    /// gets its own field name (`output_format`) AND long
    /// name (`--output-format`).
    #[arg(long, default_value = "pretty")]
    pub output_format: LintFormat,

    /// Project directory used as the scan root (defaults to the
    /// current directory). The handler resolves the nearest
    /// ancestor that contains `.specify/project.yaml`.
    #[arg(long, default_value = ".")]
    pub project_dir: PathBuf,
}

/// Clap-derivable mirror of [`DiagnosticsFormat`] per the diagnostics formatter set.
///
/// Kept distinct from the `specify-lints` enum so the standards crate
/// stays runtime-agnostic; the [`From`] impl below is the single
/// adapter. The wire spelling matches the closed diagnostics formatter set
/// (`compact`, `github`, `json`, `pretty`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LintFormat {
    /// Tab-separated one-line-per-finding shape.
    Compact,
    /// GitHub Actions workflow-annotation lines.
    Github,
    /// `DiagnosticReport` wire envelope; schema-validated before emit.
    Json,
    /// Terminal output with severity colour and source location.
    Pretty,
}

impl From<LintFormat> for DiagnosticsFormat {
    fn from(value: LintFormat) -> Self {
        match value {
            LintFormat::Compact => Self::Compact,
            LintFormat::Github => Self::Github,
            LintFormat::Json => Self::Json,
            LintFormat::Pretty => Self::Pretty,
        }
    }
}
