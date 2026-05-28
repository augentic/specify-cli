//! Clap derive surface for `specdev lint`. Mirrors
//! `src/runtime/commands/lint/cli.rs` modulo the defaults pinned by
//! RFC-34 §F2:
//!
//! - `--rules-root` defaults to `.` (the framework repo itself
//!   carries the codex tree); also reachable as the legacy
//!   `--framework-root` flag so the existing `make check` Makefile
//!   line `specdev lint --framework-root .` continues to work
//!   verbatim.
//! - `--scan-profile` is hard-coded to `framework`; no flag.
//! - `--target` is optional and defaults to the sentinel `none`
//!   string (framework scans don't have a single target adapter).
//! - `--include-core` does not exist — `CORE-*` rules are always
//!   visible to the framework run per RFC-34 §A3 / §F3.
//!
//! The per-subcommand `--output-format` flag is intentionally
//! distinct from the global `Cli::format` flag (`text` / `json`):
//! the global flag still controls the failure-envelope rendering on
//! infrastructure error, while `--output-format` selects the closed
//! diagnostics formatter set (`{ json, pretty, github, compact }`)
//! for the success body. When `--output-format` is unset the
//! handler defaults it from the global flag (`json` → `Json`,
//! `text` → `Pretty`) so the legacy `specdev lint --format json`
//! invocation keeps emitting the wire envelope.

use std::path::PathBuf;

use clap::Parser;

/// Flat argument set for `specdev lint`. Modelled as a `Parser`
/// derive so it can be embedded under the top-level `Command::Lint`
/// variant without introducing a `run` sub-verb (RFC-34 §F2's
/// examples hit the flags directly: `specdev lint --rule CORE-001`).
#[derive(Debug, Parser)]
pub struct LintAction {
    /// Framework repo root used as both rules-root and scan-root.
    /// Defaults to the current directory so a contributor in a
    /// fresh clone can run bare `specdev lint`.
    #[arg(long, env = "SPECDEV_FRAMEWORK_ROOT", alias = "rules-root", default_value = ".")]
    pub framework_root: PathBuf,

    /// Target-adapter name (kebab, optionally `<name>@v<major>`).
    /// Defaults to the literal `none` because framework scans rarely
    /// scope to one target adapter; when supplied, narrows the
    /// applicability filter the same way `specrun lint --target` does.
    #[arg(long, default_value = "none")]
    pub target: String,

    /// Source-adapter name; repeatable. Each occurrence contributes
    /// one source overlay to the resolved codex.
    #[arg(long = "source", value_name = "NAME")]
    pub sources: Vec<String>,

    /// Restrict the declarative pass to specific rule ids (debug
    /// surface from RFC-34 §F2 `specdev lint --rule CORE-001`).
    /// Repeatable; empty means "evaluate every applicable rule".
    /// Does not filter the imperative pass — authoring rule ids
    /// (`rules.schema-violation`, `skill.unknown-tool`, …) do not
    /// match the closed codex `rule-id` regex.
    #[arg(long = "rule", value_name = "RULE_ID")]
    pub rules: Vec<String>,

    /// Restrict the scan to specific artifact paths (lint scope
    /// resolution). Repeatable. Project-relative to `framework-root`.
    #[arg(long = "artifact", value_name = "PATH")]
    pub artifacts: Vec<PathBuf>,

    /// Lowercase language token; repeatable. Passed to both
    /// `build_resolved_rules` and the framework indexer.
    #[arg(long = "language", value_name = "TOKEN")]
    pub languages: Vec<String>,

    /// Emit the `WorkspaceModel` only (debug). Validates the model
    /// against `WORKSPACE_MODEL_JSON_SCHEMA` before stdout emit;
    /// skips hint evaluation entirely.
    #[arg(long)]
    pub dump_model: bool,

    /// Upgrade the reserved-hint diagnostics summary finding's
    /// severity from `optional` to `important`, contributing to a
    /// non-zero exit code per the lint exit map.
    #[arg(long)]
    pub strict_hints: bool,

    /// Output format. Closed Phase 2 set per the diagnostics
    /// formatter set: `{ json, pretty, github, compact }`. When
    /// unset, derived from the global `--format` flag: `json` →
    /// `Json`, `text` → `Pretty`.
    ///
    /// Spelled `--output-format` rather than `--format` to avoid a
    /// clap conflict with the global `--format` flag on `Cli`
    /// (text vs JSON for the failure envelope).
    #[arg(long, value_enum)]
    pub output_format: Option<LintFormat>,
}

/// Clap presentation enum shared with `specrun lint`. Re-exported
/// from the runtime tree rather than redefined: it is a clap-facing
/// presentation type (kept out of the runtime-agnostic
/// `specify-lints` crate), and both copies compile into the same
/// `specify` binary, so the canonical definition — together with its
/// `From<LintFormat> for DiagnosticsFormat` adapter — lives once in
/// `crate::runtime::commands::lint::cli`.
pub use crate::runtime::commands::lint::cli::LintFormat;
