//! `specify lint framework` handler — composes the framework's
//! imperative `Check` predicates with the declarative deterministic-hint
//! interpreter into a single [`DiagnosticReport`] envelope.
//!
//! The shared pipeline lives in [`specify_standards::lint::runner`] and
//! the shared output/journal/exit tail in [`crate::output`]; this
//! handler is thin and obeys the same `Result<()>` contract as the
//! product `lint product` handler, differing only in the framework-surface
//! config it assembles:
//!
//! 1. Resolve the framework root and load the imperative
//!    [`AuthoringContext`] (framework-root canonicalisation only — every
//!    `rules.*` check now runs through the `rules` WASI tool, so no
//!    imperative producer is wired in).
//! 2. Configure the runner for the framework surface
//!    ([`ScanProfile::Framework`], [`FrameworkToolRunner`],
//!    [`ResolverDegradation::SkipDeclarative`], `include_core: true`).
//! 3. Hand the config to the shared [`crate::output::run_lint`] kernel
//!    (via [`crate::output::emit_lint_report`]), which renders the
//!    envelope, appends one `lint-completed` event, decides the
//!    blocking exit, and owns the JSON fallback on abort.

use std::time::Instant;

use jiff::Timestamp;
use specify_diagnostics::{DiagnosticReport, Format as DiagnosticsFormat};
use specify_error::{Error, Result};
use specify_standards::ResolveInputs;
use specify_standards::framework::context::Context as AuthoringContext;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::producer::DiagnosticProducer;
use specify_standards::lint::runner::{PipelineConfig, ResolverDegradation};
use specify_workflow::config::Layout;
use specify_workflow::journal::LintScope;

use crate::output::{self, Format, LintRun};
use crate::runtime::commands::lint::cli::{FrameworkArgs, LintFormat};
use crate::runtime::commands::lint::framework_tools::FrameworkToolRunner;

/// Handler entry point dispatched from `src/runtime/commands.rs`.
///
/// Returns `Result<()>` like every runtime handler; the dispatcher maps
/// the terminal error through the shared `Exit::from(&Error)` table.
/// Always leaves a stable envelope on stdout for JSON output — the real
/// report on success, an empty all-zero envelope when the run aborts
/// before emit (via [`output::run_lint`]).
///
/// # Errors
///
/// Propagates the framework-root load error and any pipeline /
/// render abort routed through [`output::run_lint`].
pub fn run(format: Format, action: &FrameworkArgs) -> Result<()> {
    let diagnostics_format = pick_format(format, action.output_format);
    output::run_lint(diagnostics_format, || build_report(action, diagnostics_format))
}

/// Assemble the framework-surface inputs and config, then run the
/// shared pipeline + emit tail. Every `?` here is a pre-emit abort
/// that [`output::run_lint`] turns into the JSON fallback envelope.
fn build_report(
    action: &FrameworkArgs, format: DiagnosticsFormat,
) -> Result<Option<DiagnosticReport>> {
    let started_at = Instant::now();
    let authoring_ctx =
        AuthoringContext::from_framework_root(&action.framework_root).map_err(|err| {
            Error::Diag {
                code: "framework-root",
                detail: err.to_string(),
            }
        })?;
    let project_dir = authoring_ctx.framework_root().to_path_buf();

    let inputs = ResolveInputs {
        project_dir: &project_dir,
        rules_root: Some(&project_dir),
        target_adapter: &action.target,
        source_adapters: &action.sources,
        artifact_paths: &action.artifacts,
        languages: &action.languages,
        include_deprecated: false,
        include_unmatched: false,
        include_core: true,
    };

    // No imperative producer: every `rules.*` check (CORE-009 namespace
    // ownership, CORE-026 duplicate id) now runs through the `rules` WASI
    // tool via `kind: tool`, folded by the declarative pass.
    let producers: [&dyn DiagnosticProducer; 0] = [];
    let rule_filter_slice: Vec<&str> = action.rules.iter().map(String::as_str).collect();
    let tool_runner = FrameworkToolRunner::new()?;
    let config = PipelineConfig {
        profile: ScanProfile::Framework,
        dump_model: action.dump_model,
        apply_ignore_directives: true,
        rule_filter: &rule_filter_slice,
        resolver_degradation: ResolverDegradation::SkipDeclarative,
        tool_runner: &tool_runner,
        producers: &producers,
    };

    let scope = LintScope {
        target: None,
        slice: None,
        artifact: action.artifacts.first().map(|p| p.display().to_string()),
    };
    output::emit_lint_report(LintRun {
        inputs: &inputs,
        config: &config,
        format,
        layout: Layout::new(&project_dir),
        now: Timestamp::now(),
        scope,
        command_label: "specify lint framework",
        started_at,
        trailing_newline: false,
    })
}

/// Resolve the diagnostics format for the success body. Per-subcommand
/// `--output-format` wins; otherwise mirror the global `--format`
/// flag so `specify lint framework --format json` still emits the wire
/// envelope.
fn pick_format(global: Format, output_format: Option<LintFormat>) -> DiagnosticsFormat {
    if let Some(value) = output_format {
        return value.into();
    }
    match global {
        Format::Json => DiagnosticsFormat::Json,
        Format::Text => DiagnosticsFormat::Pretty,
    }
}
