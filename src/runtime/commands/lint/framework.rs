//! `specify lint framework` handler — composes the framework's
//! imperative `Check` predicates with the declarative deterministic-hint
//! interpreter into a single [`DiagnosticReport`] envelope.
//!
//! The shared pipeline lives in [`specify_standards::lint::runner`] and
//! the shared output/journal/exit tail in [`crate::output`]; this
//! handler is thin and obeys the same `Result<()>` contract as the
//! consumer `lint run` handler, differing only in the framework-surface
//! config it assembles:
//!
//! 1. Resolve the framework root and load the imperative
//!    [`AuthoringContext`].
//! 2. Wrap the imperative `Check` pass as a [`DiagnosticProducer`]
//!    ([`AuthoringProducer`]) so the runner composes it with the
//!    declarative pass and dedupes the combined set by fingerprint.
//! 3. Configure the runner for the framework surface
//!    ([`ScanProfile::Framework`], [`NoopToolRunner`],
//!    [`ResolverDegradation::SkipDeclarative`], `include_core: true`).
//! 4. Hand the config to the shared [`crate::output::run_lint`] kernel
//!    (via [`crate::output::emit_lint_report`]), which renders the
//!    envelope, appends one `lint-completed` event, decides the
//!    blocking exit, and owns the JSON fallback on abort.

use std::path::Path;
use std::time::Instant;

use specify_diagnostics::{Diagnostic, DiagnosticReport, Format as DiagnosticsFormat};
use specify_error::{Error, Result};
use specify_standards::ResolveInputs;
use specify_standards::framework::check;
use specify_standards::framework::context::Context as AuthoringContext;
use specify_standards::lint::eval::tool::{ToolOutput, ToolRunError, ToolRunner};
use specify_standards::lint::producer::DiagnosticProducer;
use specify_standards::lint::runner::{PipelineConfig, ResolverDegradation};
use specify_standards::lint::{ScanProfile, WorkspaceModel};
use specify_workflow::config::Layout;
use specify_workflow::journal::LintScope;

use crate::output::{self, Format, LintRun};
use crate::runtime::commands::lint::cli::{FrameworkArgs, LintFormat};

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

    let producer = AuthoringProducer { ctx: &authoring_ctx };
    let producers: [&dyn DiagnosticProducer; 1] = [&producer];
    let rule_filter_slice: Vec<&str> = action.rules.iter().map(String::as_str).collect();
    let tool_runner = NoopToolRunner;
    let config = PipelineConfig {
        profile: ScanProfile::Framework,
        dump_model: action.dump_model,
        strict_hints: action.strict_hints,
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

/// Imperative producer wrapping the framework's `Check` pass.
///
/// Holds the loaded [`AuthoringContext`]; ignores the `WorkspaceModel`
/// the runner threads in (the imperative predicates index their own
/// inputs from the context). [`check::run`] now finalises the batch
/// itself — building each [`Diagnostic`] via
/// [`specify_standards::framework::framework_finding`], rebasing locations
/// to project-relative form, and stamping fingerprints and ids — so
/// this producer is a thin pass-through that satisfies the
/// [`DiagnosticProducer`] contract directly.
struct AuthoringProducer<'a> {
    ctx: &'a AuthoringContext,
}

impl DiagnosticProducer for AuthoringProducer<'_> {
    fn produce(&self, _model: &WorkspaceModel, _project_dir: &Path) -> Vec<Diagnostic> {
        check::run(self.ctx)
    }
}

/// `ToolRunner` stub used by the framework run.
///
/// `specify lint framework` never has a `project.yaml` to populate a
/// tool inventory from — framework runs live alongside the codex tree
/// itself, not inside an initialised consumer project. Any `kind: tool`
/// hint a framework-applicable rule declares is reported as undeclared.
struct NoopToolRunner;

impl ToolRunner for NoopToolRunner {
    fn is_declared(&self, _tool_name: &str) -> bool {
        false
    }

    fn run(
        &self, tool_name: &str, _args: &[String], _project_dir: &Path,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        Err(ToolRunError::Runtime(format!(
            "tool {tool_name} cannot run under specify lint framework; framework runs ship without \
             a tool inventory"
        )))
    }
}
