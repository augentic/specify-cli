//! Clap derive surface for `specrun lint *`. Mirrors the
//! `RulesAction` shape in `src/runtime/commands/rules/cli.rs`.
//!
//! The per-subcommand `--format` flag is intentionally distinct from
//! the global `Cli::format` flag: global `--format` toggles JSON vs
//! text for envelope-emitting handlers and the failure path, while
//! `specrun lint --format` selects the closed RFC-32 §D6 set
//! (`{ json, pretty, github, compact }`). The handler reads its own
//! per-subcommand flag and ignores the global one for the success
//! body.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use specify_lints::lint::diagnostics::Format as DiagnosticsFormat;

#[derive(Subcommand)]
pub enum LintAction {
    /// Resolve applicable rules, build a `WorkspaceModel`,
    /// evaluate deterministic hints, and emit the RFC-28 review
    /// envelope (RFC-32 §"`specrun lint` (Phase 2 CLI)").
    Run {
        /// Codex root supplying shared `UNI-*` rules. Resolution
        /// order (RFC-32 §D7): this flag → `$RULES_ROOT` env →
        /// project's `.specify/cache/rules/` → bundled tree.
        /// Validation failure exits 2 with `rules-root-required`.
        #[arg(long, env = "RULES_ROOT")]
        rules_root: Option<PathBuf>,

        /// Target-adapter name (kebab, optionally `<name>@v<major>`).
        #[arg(long)]
        target: String,

        /// Source-adapter name; repeatable. Each occurrence
        /// contributes one source overlay to the resolved codex.
        #[arg(long = "source", value_name = "NAME")]
        sources: Vec<String>,

        /// Restrict the scan to one slice's tasks (RFC-32 §D2).
        /// Reads the slice's `tasks.md` for `Touches:` / `Produces:`
        /// paths plus `.specify/slices/<name>/**`.
        #[arg(long)]
        slice: Option<String>,

        /// Restrict the scan to specific artifact paths
        /// (RFC-32 §D2). Repeatable; composes with `--slice` (union).
        #[arg(long = "artifact", value_name = "PATH")]
        artifacts: Vec<PathBuf>,

        /// Lowercase language token; repeatable. Passed to both
        /// `specrun rules export` and the consumer indexer.
        #[arg(long = "language", value_name = "TOKEN")]
        languages: Vec<String>,

        /// Emit the `WorkspaceModel` only (debug). Validates the
        /// model against `WORKSPACE_MODEL_JSON_SCHEMA` before
        /// stdout emit; skips hint evaluation entirely.
        #[arg(long)]
        dump_model: bool,

        /// Upgrade the RFC-32 §D5 reserved-hint summary finding's
        /// severity from `optional` to `important`, which
        /// contributes to a non-zero exit code per §D8.
        #[arg(long)]
        strict_hints: bool,

        /// Output format. Closed Phase 2 set per RFC-32 §D6:
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
        output_format: LintFormat,

        /// Project directory used as the scan root (defaults to the
        /// current directory). The handler resolves the nearest
        /// ancestor that contains `.specify/project.yaml`.
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
}

/// Clap-derivable mirror of [`DiagnosticsFormat`] per RFC-32 §D6.
///
/// Kept distinct from the `specify-lints` enum so the standards crate
/// stays runtime-agnostic; the [`From`] impl below is the single
/// adapter. The wire spelling matches the RFC §D6 closed set
/// (`compact`, `github`, `json`, `pretty`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LintFormat {
    /// Tab-separated one-line-per-finding shape.
    Compact,
    /// GitHub Actions workflow-annotation lines.
    Github,
    /// RFC-28 wire envelope; schema-validated before emit.
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
