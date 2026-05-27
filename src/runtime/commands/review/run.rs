//! `specrun review run` handler — RFC-32 §"`specrun review` (Phase 2 CLI)".
//!
//! Composes the standards-layer pipeline:
//!
//! 1. Resolve `--slice` / `--artifact` scope per RFC-32 §D2.
//! 2. Build the resolved codex (`specify_codex::build_resolved_codex`)
//!    using the same artifact / language filters as the indexer so
//!    the resolved rule set and the scan set agree.
//! 3. Build the consumer `WorkspaceModel` (`review::index::build`).
//! 4. Evaluate executable deterministic hints per rule, skipping
//!    `review-mode: model-assisted` rules.
//! 5. Mint the §D5 reserved-hint summary finding.
//! 6. Render the RFC-28 envelope via `review::diagnostics::render`.
//! 7. Decide exit: any `critical | important` finding lands the
//!    process on `Exit::ValidationFailed` (code 2) per §D8.
//!
//! Every failure path routes through a closed `Error` variant so
//! `Exit::from(&Error)` lands on the §D8 exit-code map. The mapping
//! tables live next to the helpers (`map_index_error`,
//! `map_hint_error`, `map_render_error`) and are pinned by table-driven
//! tests at the bottom of this file.

use std::fs;
use std::path::{Path, PathBuf};

use specify_codex::review::diagnostics::{
    Format as DiagnosticsFormat, RenderError, ReviewResult, ReviewResultVersion, ReviewSummary,
    render,
};
use specify_codex::review::eval::tool::{ToolOutput, ToolRunError, ToolRunner};
use specify_codex::review::eval::{HintError, evaluate, reserved_hint_summary};
use specify_codex::review::index::{IndexError, build as build_model};
use specify_codex::review::{ScanProfile, WorkspaceModel};
use specify_codex::{
    ResolveInputs, ResolvedRule, ReviewFinding, ReviewMode, Severity, build_resolved_codex,
};
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};
use specify_schema::{WORKSPACE_MODEL_JSON_SCHEMA, validate_serialisable};
use specify_tool::host::{RunContext, WasiRunner};
use specify_tool::manifest::ToolScope;

use crate::runtime::commands::codex::export::map_resolve_error;
use crate::runtime::commands::tool::{Inventory, ScopedTool, build_inventory};
use crate::runtime::context::Ctx;

/// Handler entry point dispatched from `src/runtime/commands.rs`.
///
/// # Errors
///
/// Closed mapping per RFC-32 §D8 — see [`map_index_error`],
/// [`map_hint_error`], [`map_render_error`], and `map_resolve_error`.
#[expect(
    clippy::too_many_arguments,
    reason = "Arguments mirror the closed RFC-32 §`specrun review` (Phase 2 CLI) set; the handler threads the clap-derived surface verbatim through to ResolveInputs and review::index::build."
)]
pub fn run(
    ctx: &Ctx, codex_root: Option<&Path>, target: &str, sources: &[String], slice: Option<&str>,
    artifacts: &[PathBuf], languages: &[String], dump_model: bool, strict_hints: bool,
    format: DiagnosticsFormat,
) -> Result<()> {
    let artifact_set = compose_artifact_set(&ctx.project_dir, slice, artifacts)?;
    let resolved_root = resolve_codex_root(ctx, codex_root);

    let inputs = ResolveInputs {
        project_dir: &ctx.project_dir,
        codex_root: resolved_root.as_deref(),
        target_adapter: target,
        source_adapters: sources,
        artifact_paths: &artifact_set,
        languages,
        include_deprecated: false,
        include_unmatched: false,
    };
    let resolved = build_resolved_codex(&inputs).map_err(map_resolve_error)?;

    let model = build_model(&ctx.project_dir, ScanProfile::Consumer, &artifact_set, languages)
        .map_err(map_index_error)?;

    if dump_model {
        return emit_dump_model(&model);
    }

    let runner = WasiToolRunner::new(ctx)?;
    let mut findings: Vec<ReviewFinding> = Vec::new();
    let mut reserved: Vec<specify_codex::review::eval::ReservedSkipped> = Vec::new();
    let mut next_id: u64 = 1;

    for rule in &resolved.rules {
        if matches!(rule.review_mode, Some(ReviewMode::ModelAssisted)) {
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

    if let Some(summary) = reserved_hint_summary(&reserved, strict_hints) {
        findings.push(summary);
    }

    let result = ReviewResult {
        version: ReviewResultVersion,
        summary: ReviewSummary::from_findings(&findings),
        findings,
    };

    let rendered = render(format, &result).map_err(map_render_error)?;
    println!("{rendered}");

    decide_exit(&result)
}

/// Resolve the codex root per RFC-32 §D7.
///
/// Order: explicit `--codex-root` (clap-bound to `CODEX_ROOT` env)
/// → `<project_dir>/.specify/cache/codex/` when present → defer to
/// `build_resolved_codex`, which performs the monorepo probe and
/// surfaces `codex-root-required` when every step misses.
///
/// The §D7 step that walks a bundled tree alongside the binary
/// install is intentionally not implemented in v1; consumer projects
/// pin a codex via `--codex-root` or the project cache rung.
fn resolve_codex_root(ctx: &Ctx, flag: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = flag {
        return Some(path.to_path_buf());
    }
    let cache = ctx.project_dir.join(".specify").join("cache").join("codex");
    cache.is_dir().then_some(cache)
}

/// Compose the `artifact_paths` vector handed to both the resolver
/// and the indexer per RFC-32 §D2.
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
/// `Error::Diag` (exit 1) per §D8.
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

/// Decide exit per RFC-32 §D8 last row: any `critical | important`
/// finding lands `Exit::ValidationFailed` (code 2); everything else
/// returns `Ok(())` (code 0).
fn decide_exit(result: &ReviewResult) -> Result<()> {
    let elevated = result
        .findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Critical | Severity::Important));
    if !elevated {
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
            rule: "deterministic review surfaced critical/important findings".to_string(),
            detail: Some(detail),
        }],
    })
}

/// Map a `review::index::IndexError` onto the §D8 exit-code table.
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

/// Map a `review::eval::HintError` onto the §D8 exit-code table.
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

/// Map a `review::diagnostics::RenderError` onto the §D8 exit-code
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

/// `ToolRunner` impl bridging `specify-codex`'s standards-layer
/// trait to the runtime's declared WASI tool inventory.
///
/// Owns the inventory built from `project.yaml` + the active target
/// adapter's sidecar manifest so the §D4 `is_declared` / `run`
/// contract resolves without re-walking on every hint. `run`
/// delegates to [`WasiRunner::run_captured`] which mirrors
/// [`WasiRunner::run`] but redirects stdout / stderr into capped
/// memory pipes so the host can fold a tool's `ReviewResult`
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
            jiff::Timestamp::now(),
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
mod tests {
    use std::path::PathBuf;

    use specify_codex::HintKind;

    use super::*;

    fn fake_rule() -> ResolvedRule {
        ResolvedRule {
            rule_id: "UNI-001".into(),
            title: "t".into(),
            severity: Severity::Important,
            trigger: "trigger".into(),
            review_mode: None,
            applicability: None,
            deterministic_hints: None,
            references: None,
            origin: specify_codex::Origin::Shared,
            path_root: specify_codex::PathRoot::CodexRoot,
            path: "shared/UNI-001.md".into(),
            body: String::new(),
            deprecated: None,
        }
    }

    #[test]
    fn unsupported_scan_profile_maps_to_validation_exit_2() {
        let err = map_index_error(IndexError::UnsupportedScanProfile(ScanProfile::Framework));
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-unsupported-scan-profile");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn project_dir_missing_maps_to_validation_exit_2() {
        let err = map_index_error(IndexError::ProjectDirMissing(PathBuf::from("/missing")));
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-project-dir-missing");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn override_compile_maps_to_validation_exit_2() {
        let err = map_index_error(IndexError::OverrideCompile("bad glob".into()));
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-index-override-compile");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_hint_kind_maps_to_validation_exit_2() {
        let rule = fake_rule();
        let err = map_hint_error(
            &rule,
            HintError::Unsupported {
                rule_id: "UNI-001".into(),
                kind: HintKind::Unique,
                reason: "reserved",
            },
        );
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-unsupported-hint-kind");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn schema_compile_maps_to_validation_exit_2() {
        let rule = fake_rule();
        let err = map_hint_error(
            &rule,
            HintError::SchemaCompile {
                rule_id: "UNI-001".into(),
                schema_ref: "codex-rule".into(),
                detail: "compile failed".into(),
            },
        );
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-schema-compile-failed");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn schema_resolve_maps_to_validation_exit_2() {
        let rule = fake_rule();
        let err = map_hint_error(
            &rule,
            HintError::SchemaResolve {
                rule_id: "UNI-001".into(),
                schema_ref: "missing".into(),
                reason: "no such id".into(),
            },
        );
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-schema-resolve-failed");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn regex_compile_maps_to_validation_exit_2() {
        let rule = fake_rule();
        let bad: String = "(".to_string();
        let source = ::regex::Regex::new(&bad).expect_err("constructed invalid regex");
        let err = map_hint_error(
            &rule,
            HintError::RegexCompile {
                rule_id: "UNI-001".into(),
                pattern: bad,
                source,
            },
        );
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-regex-compile-failed");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn tool_invocation_maps_to_validation_exit_2() {
        let rule = fake_rule();
        let err = map_hint_error(
            &rule,
            HintError::ToolInvocation {
                rule_id: "UNI-001".into(),
                tool: "contract".into(),
                detail: "runtime".into(),
            },
        );
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-tool-invocation-failed");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn tool_undeclared_maps_to_validation_exit_2() {
        let rule = fake_rule();
        let err = map_hint_error(
            &rule,
            HintError::ToolUndeclared {
                rule_id: "UNI-001".into(),
                tool: "contract".into(),
            },
        );
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "review-tool-undeclared");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn filesystem_maps_to_filesystem_exit_1() {
        let rule = fake_rule();
        let err = map_hint_error(
            &rule,
            HintError::Filesystem {
                op: "read",
                path: PathBuf::from("/missing"),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
        );
        match err {
            Error::Filesystem { op, path, .. } => {
                assert_eq!(op, "review-eval");
                assert_eq!(path, PathBuf::from("/missing"));
            }
            other => panic!("expected Filesystem, got {other:?}"),
        }
    }

    #[test]
    fn render_schema_validation_maps_to_diag_exit_1() {
        let err = map_render_error(RenderError::JsonSchemaValidation {
            detail: "schema mismatch".into(),
        });
        match err {
            Error::Diag { code, .. } => {
                assert_eq!(code, "review-envelope-schema");
            }
            other => panic!("expected Diag, got {other:?}"),
        }
    }

    #[test]
    fn slice_tasks_parser_collects_bullet_paths() {
        let text = "## Tasks\n\n- intro\n\n## Touches\n\n- crates/billing/src/lib.rs\n* docs/billing.md\n\n## Notes\n\n- unrelated\n";
        let paths = parse_slice_tasks_paths(text);
        assert_eq!(
            paths,
            vec![PathBuf::from("crates/billing/src/lib.rs"), PathBuf::from("docs/billing.md"),]
        );
    }

    #[test]
    fn slice_tasks_parser_handles_both_touches_and_produces() {
        let text = "## Produces\n\n- a.md\n\n## Touches\n\n- b.md\n";
        let paths = parse_slice_tasks_paths(text);
        assert_eq!(paths, vec![PathBuf::from("a.md"), PathBuf::from("b.md")]);
    }
}
