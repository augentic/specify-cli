//! `specdev lint` handler — composes the framework's imperative
//! `Check` predicates with the declarative deterministic-hint
//! interpreter into a single [`LintResult`] envelope per RFC-34 §F2.
//!
//! Pipeline mirrors `src/runtime/commands/lint/run.rs`:
//!
//! 1. Resolve the framework root and load the imperative
//!    [`AuthoringContext`].
//! 2. Run the imperative pass via [`specify_authoring::check::run`]
//!    and map each [`Finding`] to [`LintFinding`] through the
//!    existing RFC-28 Phase 3 mapper at
//!    [`crate::authoring::map_finding`].
//! 3. Build the resolved codex
//!    ([`specify_lints::build_resolved_rules`]) with `include_core:
//!    true` so `CORE-*` rules participate by default per RFC-34
//!    §A3 / §F3.
//! 4. Build the framework [`WorkspaceModel`] via
//!    [`build_model`] under [`ScanProfile::Framework`].
//! 5. Evaluate executable deterministic hints (skipping
//!    `lint-mode: model-assisted` rules), mint the reserved-hint
//!    summary, apply ignore directives.
//! 6. Deduplicate the combined findings by fingerprint per RFC-34
//!    §F5 — the imperative and declarative passes may surface the
//!    same `(rule-id, location)` during migration.
//! 7. Render the envelope via [`render`] and append exactly one
//!    `lint-completed` event to `<framework_root>/.specify/journal.jsonl`
//!    per RFC-34 §F7.
//! 8. Decide exit per [`blocking_findings_present`] (lint exit map).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use jiff::Timestamp;
use specify_authoring::check;
use specify_authoring::context::Context as AuthoringContext;
use specify_authoring::exit::Exit;
use specify_authoring::finding::Finding;
use specify_domain::config::Layout;
use specify_domain::journal::{
    self, Event, EventKind, LintCompletedPayload, LintCounts, LintScope,
};
use specify_error::{Error, Result};
use specify_lints::fingerprint::fingerprint as compute_fingerprint;
use specify_lints::lint::ScanProfile;
use specify_lints::lint::diagnostics::{
    Format as DiagnosticsFormat, LintResult, LintResultVersion, LintSummary, count_status,
    emit_dump_model, map_index_error, map_render_error, render,
};
use specify_lints::lint::eval::tool::{ToolOutput, ToolRunError, ToolRunner};
use specify_lints::lint::eval::{evaluate_rules, reserved_hint_summary};
use specify_lints::lint::ignore::{apply as apply_directives, blocking_findings_present};
use specify_lints::lint::index::build as build_model;
use specify_lints::rules::ResolvedRules;
use specify_lints::{
    FindingStatus, LintFinding, ResolveInputs, build_resolved_rules, map_resolve_error,
};

use crate::authoring::commands::lint::cli::{LintAction, LintFormat};
use crate::authoring::map_finding::map_findings;
use crate::output::Format;

/// Handler entry point dispatched from `src/authoring/commands.rs`.
///
/// Always renders an envelope on stdout (an empty all-zero envelope
/// when JSON output is requested but the pipeline aborts before
/// emit) so CI consumers can rely on a stable shape regardless of
/// outcome — preserving the Phase 3 wire contract.
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

    let imperative: Vec<Finding> = check::run(&authoring_ctx);
    let mut combined: Vec<LintFinding> = map_findings(&imperative);
    rebase_locations_to_project(&mut combined, &project_dir);

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
    // Resolver failures are non-fatal under `specdev lint`: the
    // imperative `Check` pass already surfaces the codex-shape
    // violations the resolver trips over (`rules.duplicate-rule-id`,
    // `rules.schema-violation`, …). Swallowing the resolver error
    // and skipping the declarative pass keeps the framework
    // contributor's edit-loop legible — the imperative findings
    // still emit, and stderr carries the diagnostic so the resolver
    // failure isn't hidden.
    let resolved = match build_resolved_rules(&inputs) {
        Ok(resolved) => resolved,
        Err(err) => {
            eprintln!("specdev lint: declarative pass skipped: {}", map_resolve_error(err));
            empty_resolved(target.clone(), sources.clone())
        }
    };

    let model = build_model(&project_dir, ScanProfile::Framework, artifacts, languages)
        .map_err(map_index_error)?;

    if *dump_model {
        emit_dump_model(&model)?;
        return Ok(BuildOutcome::DumpedModel);
    }

    let runner = NoopToolRunner;
    let rule_filter_slice: Vec<&str> = rule_filter.iter().map(String::as_str).collect();
    let (declarative, reserved, mut next_id) = evaluate_rules(
        &resolved.rules,
        &model,
        &project_dir,
        &runner,
        next_imperative_id(&combined),
        &rule_filter_slice,
    )?;

    combined.extend(declarative);
    deduplicate_by_fingerprint(&mut combined);

    let directive_outcome =
        apply_directives(&mut combined, &model.ignore_directives, &resolved.rules, next_id);
    combined.extend(directive_outcome.synthetics);
    next_id = directive_outcome.next_id_counter;

    if let Some(summary) = reserved_hint_summary(&reserved, *strict_hints) {
        combined.push(summary);
    }
    let _ = next_id;

    let result = LintResult {
        version: LintResultVersion,
        summary: LintSummary::from_findings(&combined),
        findings: combined,
    };
    Ok(BuildOutcome::Envelope { result, project_dir })
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

/// Map a `specify_error::Error` onto the closed [`Exit`] code set per
/// the lint exit map. `specify-authoring::exit::Exit` only models
/// the codes the framework run can produce — `Success(0)`,
/// `GenericFailure(1)`, `ValidationFailed(2)` — and `Argument`
/// failures piggy-back on `ValidationFailed` to match the runtime
/// convention.
const fn exit_from_error(err: &Error) -> Exit {
    match err {
        Error::Validation { .. } | Error::Argument { .. } => Exit::ValidationFailed,
        _ => Exit::GenericFailure,
    }
}

/// Print an empty all-zero envelope on stdout when the run aborts
/// before reaching the success-path emit, but only for JSON output.
/// Matches the Phase 3 contract that CI consumers can rely on a
/// stable wire shape on infrastructure error.
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

/// Rewrite each finding's `location.path` to be project-relative
/// against `project_dir`. The imperative `Check` predicates emit
/// absolute paths anchored at the canonicalised framework root, but
/// `finding.schema.json` constrains `location.path` to project-relative
/// forward-slash strings (no leading `/`, no URL scheme, no `..`
/// segments). Rebasing here keeps the schema-validating diagnostics
/// JSON formatter from rejecting the imperative envelope on emit.
///
/// Re-fingerprints each finding after the rewrite so the stored
/// hash reflects the canonical (rebased) preimage.
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

/// Compute the next `FIND-{NNNN}` id counter after the imperative
/// pass. The imperative mapper assigns ids 1..=N in order; the
/// declarative pass must continue past N so the two pass results
/// don't collide on `id`.
fn next_imperative_id(findings: &[LintFinding]) -> u64 {
    u64::try_from(findings.len()).unwrap_or(u64::MAX).saturating_add(1)
}

/// Deduplicate `findings` by canonical fingerprint while preserving
/// first-occurrence order. RFC-34 §F5 guarantees that during the
/// migration overlap a `CORE-*` rule and its retiring imperative
/// predicate will produce byte-identical fingerprints for the same
/// `(rule-id, location)` pair; this dedupe collapses the duplicate
/// so a single envelope row survives.
fn deduplicate_by_fingerprint(findings: &mut Vec<LintFinding>) {
    let mut seen: HashSet<String> = HashSet::with_capacity(findings.len());
    findings.retain(|f| seen.insert(f.fingerprint.clone()));
}

/// Append a `lint-completed` event to `<project_dir>/.specify/journal.jsonl`
/// per RFC-34 §F7. Best-effort: telemetry I/O failures log to stderr
/// and never override the scan's exit code.
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

/// Empty resolved-codex stub used when the declarative pass is
/// skipped because the codex tree itself failed to resolve. Keeps
/// the eval loop's input type uniform.
const fn empty_resolved(target_adapter: String, source_adapters: Vec<String>) -> ResolvedRules {
    ResolvedRules {
        version: 1,
        target_adapter,
        source_adapters,
        rules: Vec::new(),
    }
}

/// `ToolRunner` stub used by the framework run.
///
/// `specdev lint` never has a `project.yaml` to populate a tool
/// inventory from — framework runs live alongside the codex tree
/// itself, not inside an initialised consumer project. Until a
/// future RFC introduces a framework-side tool inventory, any
/// `kind: tool` hint a framework-applicable rule declares is
/// reported as undeclared.
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
