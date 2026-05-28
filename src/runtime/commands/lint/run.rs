//! `specrun lint run` handler.
//!
//! Composes the standards-layer pipeline:
//!
//! 1. Resolve `--slice` / `--artifact` scope per lint scope resolution.
//! 2. Build the resolved codex (`specify_lints::build_resolved_rules`)
//!    using the same artifact / language filters as the indexer so
//!    the resolved rule set and the scan set agree.
//! 3. Build the consumer `WorkspaceModel` (`lint::index::build`).
//! 4. Evaluate executable deterministic hints per rule, skipping
//!    `lint-mode: model-assisted` rules.
//! 5. Mint the reserved-hint diagnostics reserved-hint summary finding.
//! 6. Render the `LintResult` envelope via `lint::diagnostics::render`.
//! 7. Decide exit: any `critical | important` finding lands the
//!    process on `Exit::ValidationFailed` (code 2) per lint exit mapping.
//!
//! Every failure path routes through a closed `Error` variant so
//! `Exit::from(&Error)` lands on the lint exit mapping exit-code map. The mapping
//! tables live next to the helpers (`map_index_error`,
//! `map_hint_error`, `map_render_error`) and are pinned by table-driven
//! tests in `run_tests.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use jiff::Timestamp;
use specify_domain::journal::{
    self, Event, EventKind, LintCompletedPayload, LintCounts, LintScope,
};
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};
use specify_lints::lint::diagnostics::{
    Format as DiagnosticsFormat, LintResult, LintResultVersion, LintSummary, RenderError, render,
};
use specify_lints::lint::eval::tool::{ToolOutput, ToolRunError, ToolRunner};
use specify_lints::lint::eval::{HintError, evaluate, reserved_hint_summary};
use specify_lints::lint::ignore::{apply as apply_directives, blocking_findings_present};
use specify_lints::lint::index::{IndexError, build as build_model};
use specify_lints::lint::{ScanProfile, WorkspaceModel};
use specify_lints::{
    FindingStatus, LintFinding, LintMode, ResolveInputs, ResolvedRule, build_resolved_rules,
};
use specify_schema::{WORKSPACE_MODEL_JSON_SCHEMA, validate_serialisable};
use specify_tool::host::{RunContext, WasiRunner};
use specify_tool::manifest::ToolScope;

use crate::runtime::commands::rules::export::map_resolve_error;
use crate::runtime::commands::tool::{Inventory, ScopedTool, build_inventory};
use crate::runtime::context::Ctx;

/// Handler entry point dispatched from `src/runtime/commands.rs`.
///
/// # Errors
///
/// Closed mapping per lint exit mapping — see [`map_index_error`],
/// [`map_hint_error`], [`map_render_error`], and `map_resolve_error`.
#[expect(
    clippy::too_many_arguments,
    reason = "Arguments mirror the closed `specrun lint run` argument set; the handler threads the clap-derived surface verbatim through to ResolveInputs and lint::index::build."
)]
pub fn run(
    ctx: &Ctx, rules_root: Option<&Path>, target: &str, sources: &[String], slice: Option<&str>,
    artifacts: &[PathBuf], languages: &[String], dump_model: bool, strict_hints: bool,
    format: DiagnosticsFormat,
) -> Result<()> {
    let started_at = Instant::now();
    let artifact_set = compose_artifact_set(&ctx.project_dir, slice, artifacts)?;
    let resolved_root = resolve_rules_root(ctx, rules_root);

    let inputs = ResolveInputs {
        project_dir: &ctx.project_dir,
        rules_root: resolved_root.as_deref(),
        target_adapter: target,
        source_adapters: sources,
        artifact_paths: &artifact_set,
        languages,
        include_deprecated: false,
        include_unmatched: false,
    };
    let resolved = build_resolved_rules(&inputs).map_err(map_resolve_error)?;

    let model = build_model(&ctx.project_dir, ScanProfile::Consumer, &artifact_set, languages)
        .map_err(map_index_error)?;

    if dump_model {
        return emit_dump_model(&model);
    }

    let runner = WasiToolRunner::new(ctx)?;
    let mut findings: Vec<LintFinding> = Vec::new();
    let mut reserved: Vec<specify_lints::lint::eval::ReservedSkipped> = Vec::new();
    let mut next_id: u64 = 1;

    for rule in &resolved.rules {
        if matches!(rule.lint_mode, Some(LintMode::ModelAssisted)) {
            continue;
        }
        let Some(hints) = rule.deterministic_hints.as_deref() else {
            continue;
        };
        if hints.is_empty() {
            continue;
        }
        let outcome = evaluate(rule, hints, &model, &ctx.project_dir, &runner, next_id)
            .map_err(|err| map_hint_error(rule, err))?;
        findings.extend(outcome.findings);
        reserved.extend(outcome.reserved_skipped);
        next_id = outcome.next_id_counter;
    }

    let outcome =
        apply_directives(&mut findings, &model.ignore_directives, &resolved.rules, next_id);
    findings.extend(outcome.synthetics);
    next_id = outcome.next_id_counter;

    if let Some(summary) = reserved_hint_summary(&reserved, strict_hints) {
        findings.push(summary);
    }
    // `next_id` is intentionally not consumed further in v1; future
    // post-passes (RFC-33b baseline matching, telemetry IDs) will
    // continue to thread it.
    let _ = next_id;

    let result = LintResult {
        version: LintResultVersion,
        summary: LintSummary::from_findings(&findings),
        findings,
    };

    let rendered = render(format, &result).map_err(map_render_error)?;
    println!("{rendered}");

    let exit_code: i32 = if blocking_findings_present(&result.findings) { 2 } else { 0 };
    emit_lint_completed(
        ctx,
        target,
        slice,
        artifacts,
        &result.findings,
        started_at.elapsed().as_millis(),
        exit_code,
    );
    decide_exit(&result)
}

/// Resolve the rules root per rules-root resolution.
///
/// Order: explicit `--rules-root` (clap-bound to `RULES_ROOT` env)
/// → `<project_dir>/.specify/cache/rules/` when present → defer to
/// `build_resolved_rules`, which performs the monorepo probe and
/// surfaces `rules-root-required` when every step misses.
///
/// The rules-root resolution step that walks a bundled tree alongside the binary
/// install is intentionally not implemented in v1; consumer projects
/// pin a codex via `--rules-root` or the project cache rung.
fn resolve_rules_root(ctx: &Ctx, flag: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = flag {
        return Some(path.to_path_buf());
    }
    let cache = ctx.project_dir.join(".specify").join("cache").join("rules");
    cache.is_dir().then_some(cache)
}

/// Compose the `artifact_paths` vector handed to both the resolver
/// and the indexer per lint scope resolution.
///
/// - `--slice <name>` contributes every path listed under the slice's
///   `tasks.md` `Touches:` / `Produces:` headings plus the slice
///   directory `.specify/slices/<name>` itself.
/// - `--artifact <path>` entries are appended verbatim.
/// - Empty result means "full scan" — both the resolver and indexer
///   read `&[]` that way.
fn compose_artifact_set(
    project_dir: &Path, slice: Option<&str>, artifacts: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(slice_name) = slice {
        let tasks_path =
            project_dir.join(".specify").join("slices").join(slice_name).join("tasks.md");
        match fs::read_to_string(&tasks_path) {
            Ok(text) => out.extend(parse_slice_tasks_paths(&text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Validation {
                    results: vec![ValidationSummary {
                        status: ValidationStatus::Fail,
                        rule_id: "review-slice-tasks-missing".to_string(),
                        rule: format!("slice {slice_name} has no tasks.md"),
                        detail: Some(tasks_path.display().to_string()),
                    }],
                });
            }
            Err(err) => {
                return Err(Error::Filesystem {
                    op: "review-slice-read",
                    path: tasks_path,
                    source: err,
                });
            }
        }
        out.push(PathBuf::from(format!(".specify/slices/{slice_name}")));
    }
    out.extend(artifacts.iter().cloned());
    Ok(out)
}

/// Extract bullet-list paths from the `Touches:` / `Produces:`
/// headings of a slice's `tasks.md`.
///
/// Tolerates `- path` and `* path` bullet markers; ignores blank
/// lines and prose; stops collecting at the next `## ` heading.
fn parse_slice_tasks_paths(text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("##") {
            let heading = rest.trim_start_matches(' ').trim();
            inside = matches!(heading.to_ascii_lowercase().as_str(), "touches" | "produces");
            continue;
        }
        if !inside {
            continue;
        }
        let bullet =
            trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).map(str::trim);
        if let Some(path) = bullet
            && !path.is_empty()
        {
            out.push(PathBuf::from(path));
        }
    }
    out
}

/// Serialise the model, validate it against the v1 schema, and print
/// it to stdout. Validation failure is an internal bug — wrapped as
/// `Error::Diag` (exit 1) per lint exit mapping.
fn emit_dump_model(model: &WorkspaceModel) -> Result<()> {
    validate_serialisable(
        model,
        WORKSPACE_MODEL_JSON_SCHEMA,
        "review-dump-model-schema",
        "WorkspaceModel matches workspace-model.schema.json",
        "review-dump-model-serialise",
        "WorkspaceModel",
    )?;
    let rendered = serde_json::to_string_pretty(model).map_err(|err| Error::Diag {
        code: "review-dump-model-serialise",
        detail: format!("failed to serialise WorkspaceModel: {err}"),
    })?;
    println!("{rendered}");
    Ok(())
}

/// Build a [`LintCompletedPayload`] from the final finding set and
/// append it to the project journal per RFC-33a §"Journal event"
/// (D8). Best-effort: serialise/IO failures are logged to stderr and
/// swallowed so a telemetry hiccup never overrides the scan's exit
/// code (mirrors the safety stance documented on the variant
/// itself).
///
/// `baseline_present` is hard-coded `false`; RFC-33b makes it
/// scan-derived when it lands.
fn emit_lint_completed(
    ctx: &Ctx, target: &str, slice: Option<&str>, artifacts: &[PathBuf], findings: &[LintFinding],
    duration_ms: u128, exit_code: i32,
) {
    let scope = LintScope {
        target: (!target.is_empty()).then(|| target.to_string()),
        slice: slice.map(str::to_string),
        // Populate `artifact` only when the scan was narrowed to
        // exactly one path; multi-artifact and full scans leave the
        // field `null` per the variant doc.
        artifact: (artifacts.len() == 1).then(|| artifacts[0].display().to_string()),
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
    if let Err(err) = journal::append_batch(ctx.layout(), std::slice::from_ref(&event)) {
        eprintln!("specrun lint: failed to append lint-completed journal event: {err}");
    }
}

/// Count findings whose `status` matches `target`. Passing `None`
/// counts the `open` bucket per RFC-33a — an unset `status` is
/// treated as `Open`, matching the status-aware exit predicate in
/// [`specify_lints::lint::ignore::blocking_findings_present`].
fn count_status(findings: &[LintFinding], target: Option<FindingStatus>) -> u32 {
    let count = findings
        .iter()
        .filter(|f| {
            target.map_or_else(
                || matches!(f.status, None | Some(FindingStatus::Open)),
                |want| f.status == Some(want),
            )
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Decide exit per RFC-33a §"Exit and presentation semantics": exit 2
/// only when at least one finding carries `status: open` AND
/// `severity ∈ {critical, important}`. Findings demoted to `ignored`
/// or `false-positive` by the directive pass remain in the envelope
/// but do not block.
fn decide_exit(result: &LintResult) -> Result<()> {
    if !blocking_findings_present(&result.findings) {
        return Ok(());
    }
    let detail = format!(
        "critical={} important={} suggestion={} optional={}",
        result.summary.critical,
        result.summary.important,
        result.summary.suggestion,
        result.summary.optional,
    );
    Err(Error::Validation {
        results: vec![ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: "review-findings-present".to_string(),
            rule: "deterministic review surfaced open critical/important findings".to_string(),
            detail: Some(detail),
        }],
    })
}

/// Map a `lint::index::IndexError` onto the lint exit mapping exit-code table.
///
/// | `IndexError`                | `Error` variant                            | Exit |
/// |-----------------------------|--------------------------------------------|------|
/// | `UnsupportedScanProfile`    | `Validation { review-unsupported-scan-profile }` | 2 |
/// | `ProjectDirMissing`         | `Validation { review-project-dir-missing }`      | 2 |
/// | `OverrideCompile`           | `Validation { review-index-override-compile }`   | 2 |
fn map_index_error(err: IndexError) -> Error {
    match err {
        IndexError::UnsupportedScanProfile(profile) => Error::validation_failed(
            "review-unsupported-scan-profile",
            "v1 supports only scan_profile: consumer",
            format!("requested scan profile: {profile:?}"),
        ),
        IndexError::ProjectDirMissing(path) => Error::validation_failed(
            "review-project-dir-missing",
            "project directory does not exist",
            path.display().to_string(),
        ),
        IndexError::OverrideCompile(detail) => Error::validation_failed(
            "review-index-override-compile",
            "always-ignore override pattern failed to compile",
            detail,
        ),
        other => Error::Diag {
            code: "review-index",
            detail: other.to_string(),
        },
    }
}

/// Map a `lint::eval::HintError` onto the lint exit mapping exit-code table.
///
/// | `HintError`        | `Error` variant                                  | Exit |
/// |--------------------|--------------------------------------------------|------|
/// | `Unsupported`      | `Validation { review-unsupported-hint-kind }`    | 2    |
/// | `SchemaCompile`    | `Validation { review-schema-compile-failed }`    | 2    |
/// | `SchemaResolve`    | `Validation { review-schema-resolve-failed }`    | 2    |
/// | `RegexCompile`     | `Validation { review-regex-compile-failed }`     | 2    |
/// | `ToolInvocation`   | `Validation { review-tool-invocation-failed }`   | 2    |
/// | `ToolUndeclared`   | `Validation { review-tool-undeclared }`          | 2    |
/// | `Filesystem`       | `Filesystem { op: "review-eval" }`               | 1    |
fn map_hint_error(rule: &ResolvedRule, err: HintError) -> Error {
    match err {
        HintError::Unsupported {
            rule_id,
            kind,
            reason,
        } => Error::validation_failed(
            "review-unsupported-hint-kind",
            format!("rule {rule_id}: hint kind {kind:?} is not supported in v1"),
            reason.to_string(),
        ),
        HintError::SchemaCompile {
            rule_id,
            schema_ref,
            detail,
        } => Error::validation_failed(
            "review-schema-compile-failed",
            format!("rule {rule_id}: schema {schema_ref} failed to compile"),
            detail,
        ),
        HintError::SchemaResolve {
            rule_id,
            schema_ref,
            reason,
        } => Error::validation_failed(
            "review-schema-resolve-failed",
            format!("rule {rule_id}: schema {schema_ref} could not be resolved"),
            reason,
        ),
        HintError::RegexCompile {
            rule_id,
            pattern,
            source,
        } => Error::validation_failed(
            "review-regex-compile-failed",
            format!("rule {rule_id}: regex {pattern} failed to compile"),
            source.to_string(),
        ),
        HintError::ToolInvocation {
            rule_id,
            tool,
            detail,
        } => Error::validation_failed(
            "review-tool-invocation-failed",
            format!("rule {rule_id}: tool {tool} invocation failed"),
            detail,
        ),
        HintError::ToolUndeclared { rule_id, tool } => Error::validation_failed(
            "review-tool-undeclared",
            format!("rule {rule_id}: tool {tool} not declared by the project"),
            format!("declare {tool} in tools.yaml or remove the hint (rule path: {})", rule.path),
        ),
        HintError::Filesystem { op, path, source } => {
            let _ = op;
            Error::Filesystem {
                op: "review-eval",
                path,
                source,
            }
        }
    }
}

/// Map a `lint::diagnostics::RenderError` onto the lint exit mapping exit-code
/// table. Both variants are internal bugs (the typed envelope cannot
/// legally fail v1 schema validation or JSON serialisation); the
/// mapping exists so the failure surface is uniform.
///
/// | `RenderError`              | `Error` variant                             | Exit |
/// |----------------------------|---------------------------------------------|------|
/// | `JsonSchemaValidation`     | `Diag { review-envelope-schema }`           | 1    |
/// | `JsonSerialise`            | `Diag { review-envelope-serialise }`        | 1    |
fn map_render_error(err: RenderError) -> Error {
    match err {
        RenderError::JsonSchemaValidation { detail } => Error::Diag {
            code: "review-envelope-schema",
            detail,
        },
        RenderError::JsonSerialise(source) => Error::Diag {
            code: "review-envelope-serialise",
            detail: source.to_string(),
        },
        other => Error::Diag {
            code: "review-envelope",
            detail: other.to_string(),
        },
    }
}

/// `ToolRunner` impl bridging `specify-lints`'s standards-layer
/// trait to the runtime's declared WASI tool inventory.
///
/// Owns the inventory built from `project.yaml` + the active target
/// adapter's sidecar manifest so the `kind: tool` evaluator contract `is_declared` / `run`
/// contract resolves without re-walking on every hint. `run`
/// delegates to [`WasiRunner::run_captured`] which mirrors
/// [`WasiRunner::run`] but redirects stdout / stderr into capped
/// memory pipes so the host can fold a tool's `LintResult`
/// envelope into the scan output.
struct WasiToolRunner {
    project_dir: PathBuf,
    inventory: Inventory,
    runner: WasiRunner,
}

impl WasiToolRunner {
    fn new(ctx: &Ctx) -> Result<Self> {
        let inventory = build_inventory(ctx)?;
        let runner = WasiRunner::new()?;
        Ok(Self {
            project_dir: ctx.project_dir.clone(),
            inventory,
            runner,
        })
    }

    fn lookup<'a>(&'a self, name: &str) -> Option<&'a ScopedTool> {
        self.inventory.find(name)
    }
}

impl ToolRunner for WasiToolRunner {
    fn is_declared(&self, tool_name: &str) -> bool {
        self.lookup(tool_name).is_some()
    }

    fn run(
        &self, tool_name: &str, args: &[String], project_dir: &Path,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let Some(scoped) = self.lookup(tool_name) else {
            return Err(ToolRunError::Runtime(format!("tool {tool_name} is not declared")));
        };
        let resolved = specify_tool::resolver::resolve(
            scoped.scope(),
            scoped.tool(),
            Timestamp::now(),
            &self.project_dir,
        )
        .map_err(|err| ToolRunError::Runtime(err.to_string()))?;
        let mut run_ctx = RunContext::new(project_dir, args.to_vec());
        if let ToolScope::Plugin { capability_dir, .. } = scoped.scope() {
            run_ctx = run_ctx.with_capability_dir(capability_dir);
        }
        let captured = self
            .runner
            .run_captured(&resolved, &run_ctx)
            .map_err(|err| ToolRunError::Runtime(err.to_string()))?;
        Ok(ToolOutput {
            stdout: captured.stdout,
            stderr: captured.stderr,
            exit_code: captured.exit_code,
        })
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
