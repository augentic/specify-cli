//! `specdev lint` handler — composes the framework's imperative
//! `Check` predicates with the declarative deterministic-hint
//! interpreter into a single [`LintResult`] envelope.
//!
//! The shared pipeline lives in [`specify_lints::lint::runner`]; this
//! handler is thin:
//!
//! 1. Resolve the framework root and load the imperative
//!    [`AuthoringContext`].
//! 2. Wrap the imperative `Check` pass as a [`DiagnosticProducer`]
//!    ([`AuthoringProducer`]) so the runner composes it with the
//!    declarative pass and dedupes the combined set by fingerprint.
//! 3. Configure the runner for the framework surface
//!    ([`ScanProfile::Framework`], [`NoopToolRunner`],
//!    [`ResolverDegradation::SkipDeclarative`], `include_core: true`).
//! 4. Render the envelope and append exactly one `lint-completed`
//!    event to `<framework_root>/.specify/journal.jsonl`.
//! 5. Decide exit per [`blocking_findings_present`].

use std::path::{Path, PathBuf};
use std::time::Instant;

use jiff::Timestamp;
use specify_authoring::check;
use specify_authoring::context::Context as AuthoringContext;
use specify_authoring::exit::Exit;
use specify_error::{Error, Result};
use specify_lints::fingerprint::fingerprint as compute_fingerprint;
use specify_lints::lint::diagnostics::{
    Format as DiagnosticsFormat, LintResult, LintResultVersion, LintSummary, count_status,
    map_render_error, render,
};
use specify_lints::lint::eval::tool::{ToolOutput, ToolRunError, ToolRunner};
use specify_lints::lint::ignore::blocking_findings_present;
use specify_lints::lint::producer::DiagnosticProducer;
use specify_lints::lint::runner::{
    PipelineConfig, ResolverDegradation, RunOutcome, run as run_pipeline,
};
use specify_lints::lint::{ScanProfile, WorkspaceModel};
use specify_lints::{FindingStatus, LintFinding, ResolveInputs};
use specify_workflow::config::Layout;
use specify_workflow::journal::{
    self, Event, EventKind, LintCompletedPayload, LintCounts, LintScope,
};

use crate::authoring::commands::lint::cli::{LintAction, LintFormat};
use crate::authoring::map_finding::map_findings;
use crate::output::Format;

/// Handler entry point dispatched from `src/authoring/commands.rs`.
///
/// Always renders an envelope on stdout (an empty all-zero envelope
/// when JSON output is requested but the pipeline aborts before
/// emit) so CI consumers can rely on a stable shape regardless of
/// outcome.
pub fn run(format: Format, action: &LintAction) -> Exit {
    let started_at = Instant::now();
    let diagnostics_format = pick_format(format, action.output_format);

    match build_envelope(action) {
        Ok(BuildOutcome::Envelope { result, project_dir }) => {
            let rendered = match render(diagnostics_format, &result) {
                Ok(rendered) => rendered,
                Err(err) => {
                    let err = map_render_error(err);
                    eprintln!("error: {err}");
                    emit_fallback_envelope(diagnostics_format);
                    return exit_from_error(&err);
                }
            };
            print!("{rendered}");
            let exit_code: i32 = if blocking_findings_present(&result.findings) { 2 } else { 0 };
            emit_lint_completed(
                &project_dir,
                action.artifacts.first().map(PathBuf::as_path),
                &result.findings,
                started_at.elapsed().as_millis(),
                exit_code,
            );
            if blocking_findings_present(&result.findings) {
                Exit::ValidationFailed
            } else {
                Exit::Success
            }
        }
        Ok(BuildOutcome::DumpedModel) => Exit::Success,
        Err(err) => {
            eprintln!("error: {err}");
            emit_fallback_envelope(diagnostics_format);
            exit_from_error(&err)
        }
    }
}

/// Outcome of [`build_envelope`]: either a fully composed
/// [`LintResult`] ready to render, or the `--dump-model` shortcut
/// which has already emitted its own stdout body.
enum BuildOutcome {
    Envelope { result: LintResult, project_dir: PathBuf },
    DumpedModel,
}

fn build_envelope(action: &LintAction) -> Result<BuildOutcome> {
    let LintAction {
        framework_root,
        target,
        sources,
        rules: rule_filter,
        artifacts,
        languages,
        dump_model,
        strict_hints,
        ..
    } = action;

    let authoring_ctx =
        AuthoringContext::from_framework_root(framework_root).map_err(|err| Error::Diag {
            code: "specdev-framework-root",
            detail: err.to_string(),
        })?;
    let project_dir = authoring_ctx.framework_root().to_path_buf();

    let inputs = ResolveInputs {
        project_dir: &project_dir,
        rules_root: Some(&project_dir),
        target_adapter: target,
        source_adapters: sources,
        artifact_paths: artifacts,
        languages,
        include_deprecated: false,
        include_unmatched: false,
        include_core: true,
    };

    let producer = AuthoringProducer { ctx: &authoring_ctx };
    let producers: [&dyn DiagnosticProducer; 1] = [&producer];
    let rule_filter_slice: Vec<&str> = rule_filter.iter().map(String::as_str).collect();
    let tool_runner = NoopToolRunner;
    let config = PipelineConfig {
        profile: ScanProfile::Framework,
        dump_model: *dump_model,
        strict_hints: *strict_hints,
        apply_ignore_directives: true,
        rule_filter: &rule_filter_slice,
        resolver_degradation: ResolverDegradation::SkipDeclarative,
        tool_runner: &tool_runner,
        producers: &producers,
    };

    match run_pipeline(&inputs, &config)? {
        RunOutcome::DumpedModel => Ok(BuildOutcome::DumpedModel),
        RunOutcome::Report(result) => Ok(BuildOutcome::Envelope { result, project_dir }),
    }
}

/// Resolve the diagnostics format for the success body. Per-subcommand
/// `--output-format` wins; otherwise mirror the global `--format`
/// flag so the legacy `specdev lint --format json` invocation still
/// emits the wire envelope.
fn pick_format(global: Format, output_format: Option<LintFormat>) -> DiagnosticsFormat {
    if let Some(value) = output_format {
        return value.into();
    }
    match global {
        Format::Json => DiagnosticsFormat::Json,
        Format::Text => DiagnosticsFormat::Pretty,
    }
}

/// Map a `specify_error::Error` onto the closed [`Exit`] code set.
/// `specify-authoring::exit::Exit` only models the codes the framework
/// run can produce — `Success(0)`, `GenericFailure(1)`,
/// `ValidationFailed(2)` — and `Argument` failures piggy-back on
/// `ValidationFailed` to match the runtime convention.
const fn exit_from_error(err: &Error) -> Exit {
    match err {
        Error::Validation { .. } | Error::Argument { .. } => Exit::ValidationFailed,
        _ => Exit::GenericFailure,
    }
}

/// Print an empty all-zero envelope on stdout when the run aborts
/// before reaching the success-path emit, but only for JSON output.
fn emit_fallback_envelope(format: DiagnosticsFormat) {
    if !matches!(format, DiagnosticsFormat::Json) {
        return;
    }
    let result = LintResult {
        version: LintResultVersion,
        summary: LintSummary::default(),
        findings: Vec::new(),
    };
    match render(format, &result) {
        Ok(rendered) => println!("{rendered}"),
        Err(err) => eprintln!("error: failed to render fallback envelope: {err}"),
    }
}

/// Imperative producer wrapping the framework's `Check` pass.
///
/// Holds the loaded [`AuthoringContext`]; ignores the `WorkspaceModel`
/// the runner threads in (the imperative predicates index their own
/// inputs from the context). Maps each [`specify_authoring::finding::Finding`]
/// to a [`LintFinding`] and rebases locations to project-relative form
/// so the schema-validating JSON formatter accepts the envelope.
struct AuthoringProducer<'a> {
    ctx: &'a AuthoringContext,
}

impl DiagnosticProducer for AuthoringProducer<'_> {
    fn produce(&self, _model: &WorkspaceModel, project_dir: &Path) -> Vec<LintFinding> {
        let imperative = check::run(self.ctx);
        let mut findings = map_findings(&imperative);
        rebase_locations_to_project(&mut findings, project_dir);
        findings
    }
}

/// Rewrite each finding's `location.path` to be project-relative
/// against `project_dir`. The imperative `Check` predicates emit
/// absolute paths anchored at the canonicalised framework root, but
/// `finding.schema.json` constrains `location.path` to project-relative
/// forward-slash strings. Rebasing here keeps the schema-validating
/// JSON formatter from rejecting the imperative envelope on emit.
///
/// Re-fingerprints each finding after the rewrite so the stored hash
/// reflects the canonical (rebased) preimage.
fn rebase_locations_to_project(findings: &mut [LintFinding], project_dir: &Path) {
    let prefix = project_dir.to_string_lossy().replace('\\', "/");
    for finding in findings {
        let Some(location) = finding.location.as_mut() else {
            continue;
        };
        let normalised = location.path.replace('\\', "/");
        if let Some(rest) = normalised.strip_prefix(&prefix) {
            location.path = rest.trim_start_matches('/').to_string();
        } else {
            location.path = normalised;
        }
        finding.fingerprint = compute_fingerprint(finding);
    }
}

/// Append a `lint-completed` event to `<project_dir>/.specify/journal.jsonl`.
/// Best-effort: telemetry I/O failures log to stderr and never
/// override the scan's exit code.
fn emit_lint_completed(
    project_dir: &Path, single_artifact: Option<&Path>, findings: &[LintFinding],
    duration_ms: u128, exit_code: i32,
) {
    let scope = LintScope {
        target: None,
        slice: None,
        artifact: single_artifact.map(|p| p.display().to_string()),
    };
    let counts = LintCounts {
        open: count_status(findings, None),
        ignored: count_status(findings, Some(FindingStatus::Ignored)),
        false_positive: count_status(findings, Some(FindingStatus::FalsePositive)),
    };
    let payload = LintCompletedPayload {
        scope,
        duration_ms: u64::try_from(duration_ms).unwrap_or(u64::MAX),
        counts,
        baseline_present: false,
        exit_code,
    };
    let event = Event::new(Timestamp::now(), EventKind::LintCompleted(payload));
    let layout = Layout::new(project_dir);
    if let Err(err) = journal::append_batch(layout, std::slice::from_ref(&event)) {
        eprintln!("specdev lint: failed to append lint-completed journal event: {err}");
    }
}

/// `ToolRunner` stub used by the framework run.
///
/// `specdev lint` never has a `project.yaml` to populate a tool
/// inventory from — framework runs live alongside the codex tree
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
            "tool {tool_name} cannot run under specdev lint; framework runs ship without a tool inventory"
        )))
    }
}
