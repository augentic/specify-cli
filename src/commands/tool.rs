//! `specify tool {run,list,fetch,show,gc}` handlers.

pub(crate) mod cli;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use specify_capability::{Capability, ResolvedCapability};
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};
use specify_tool::cache::{self, CacheStatus};
use specify_tool::host::{RunContext, WasiRunner};
use specify_tool::load::{self, Warning};
use specify_tool::validate::ValidationResult as ToolValidationResult;
use specify_tool::{Tool, ToolManifest, ToolPermissions, ToolScope};

use crate::cli::OutputFormat;
use crate::context::Ctx;
use crate::output::{CliResult, Render, Stream, emit};

type CacheKey = (String, String, String);

#[derive(Debug, Clone)]
struct ScopedTool {
    scope: ToolScope,
    tool: Tool,
}

#[derive(Debug)]
struct Inventory {
    tools: Vec<ScopedTool>,
    warnings: Vec<WarningRow>,
    scopes: Vec<ToolScope>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WarningRow {
    code: &'static str,
    name: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ToolRow {
    name: String,
    version: String,
    source: String,
    scope: String,
    scope_detail: String,
    cache_status: String,
    cached_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ToolFetchRow {
    #[serde(flatten)]
    row: ToolRow,
    fetched: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ToolShowRow {
    #[serde(flatten)]
    row: ToolRow,
    permissions: ToolPermissions,
    sha256: Option<String>,
    fetched_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ListBody {
    tools: Vec<ToolRow>,
    warnings: Vec<WarningRow>,
}

impl Render for ListBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.tools.is_empty() {
            writeln!(w, "No declared tools.")?;
            return Ok(());
        }
        writeln!(w, "name\tversion\tscope\tcache\tcached path")?;
        for row in &self.tools {
            writeln!(
                w,
                "{}\t{}\t{}:{}\t{}\t{}",
                row.name,
                row.version,
                row.scope,
                row.scope_detail,
                row.cache_status,
                row.cached_path
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct FetchBody {
    tools: Vec<ToolFetchRow>,
    warnings: Vec<WarningRow>,
}

impl Render for FetchBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.tools.is_empty() {
            writeln!(w, "No declared tools to fetch.")?;
            return Ok(());
        }
        for row in &self.tools {
            let action = if row.fetched { "fetched" } else { "cached" };
            writeln!(
                w,
                "{action}: {} {} [{}:{}] {}",
                row.row.name,
                row.row.version,
                row.row.scope,
                row.row.scope_detail,
                row.row.cached_path
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody {
    tool: ToolShowRow,
    warnings: Vec<WarningRow>,
}

impl Render for ShowBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let row = &self.tool;
        writeln!(w, "name: {}", row.row.name)?;
        writeln!(w, "version: {}", row.row.version)?;
        writeln!(w, "source: {}", row.row.source)?;
        writeln!(w, "scope: {}:{}", row.row.scope, row.row.scope_detail)?;
        writeln!(w, "cache: {}", row.row.cache_status)?;
        writeln!(w, "cached path: {}", row.row.cached_path)?;
        if let Some(fetched_at) = &row.fetched_at {
            writeln!(w, "fetched at: {fetched_at}")?;
        }
        if let Some(sha256) = &row.sha256 {
            writeln!(w, "sha256: {sha256}")?;
        }
        writeln!(w, "permissions:")?;
        writeln!(w, "  read: {}", format_permission_list(&row.permissions.read))?;
        writeln!(w, "  write: {}", format_permission_list(&row.permissions.write))?;
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct GcBody {
    removed: Vec<String>,
    warnings: Vec<WarningRow>,
}

impl Render for GcBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            w,
            "Removed {} tool cache entrie(s) from current-project scopes.",
            self.removed.len()
        )?;
        for path in &self.removed {
            writeln!(w, "  {path}")?;
        }
        Ok(())
    }
}

/// Run a declared WASI tool through the concrete WASI host.
pub(crate) fn run(ctx: &Ctx, name: String, args: Vec<String>) -> Result<CliResult> {
    let inventory = build_inventory(ctx)?;
    emit_warnings_to_stderr(&inventory.warnings);
    let scoped = find(&inventory, &name)?;
    let resolved = specify_tool::resolver::resolve(&scoped.scope, &scoped.tool, Utc::now())?;
    let mut run_ctx = RunContext::new(&ctx.project_dir, args);
    if let ToolScope::Capability { capability_dir, .. } = &scoped.scope {
        run_ctx = run_ctx.with_capability_dir(capability_dir);
    }
    let runner = WasiRunner::new()?;
    let exit = runner.run(&resolved, &run_ctx)?;
    let code = u8::try_from(exit.clamp(0, 255)).expect("tool exit code is clamped to u8 range");
    Ok(if code == 0 { CliResult::Success } else { CliResult::Code(code) })
}

/// List the merged tool declarations for the current project.
pub(crate) fn list(ctx: &Ctx) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let rows = rows_for(&inventory.tools)?;
    let body = ListBody {
        tools: rows,
        warnings: inventory.warnings,
    };
    emit(Stream::Stdout, ctx.format, &body)?;
    if matches!(ctx.format, OutputFormat::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}

/// Fetch one declared tool, or all declared tools when no name is supplied.
pub(crate) fn fetch(ctx: &Ctx, name: Option<String>) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let selected = select(&inventory, name.as_deref())?;
    let mut rows = Vec::with_capacity(selected.len());
    for scoped in selected {
        let before = cache_status_for(scoped)?;
        specify_tool::resolver::resolve(&scoped.scope, &scoped.tool, Utc::now())?;
        rows.push(ToolFetchRow {
            row: row_for(scoped)?,
            fetched: before != CacheStatus::Hit,
        });
    }

    let body = FetchBody {
        tools: rows,
        warnings: inventory.warnings,
    };
    emit(Stream::Stdout, ctx.format, &body)?;
    if matches!(ctx.format, OutputFormat::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}

/// Show one declared tool's metadata and cache state.
pub(crate) fn show(ctx: &Ctx, name: String) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let scoped = find(&inventory, &name)?;
    let row = show_row_for(scoped)?;
    let body = ShowBody {
        tool: row,
        warnings: inventory.warnings,
    };
    emit(Stream::Stdout, ctx.format, &body)?;
    if matches!(ctx.format, OutputFormat::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}

/// Remove cache entries not referenced by the current project's merged tool list.
pub(crate) fn gc(ctx: &Ctx) -> Result<()> {
    let inventory = build_inventory(ctx)?;
    let mut kept_by_scope = kept_by_scope(&inventory);
    let mut removed = Vec::new();
    for scope in &inventory.scopes {
        let kept = kept_by_scope.remove(scope).unwrap_or_default();
        for path in cache::scan_for_gc(scope, &kept)? {
            fs::remove_dir_all(&path).map_err(|err| Error::Diag {
                code: "tool-cache-remove-failed",
                detail: format!("failed to remove tool cache directory {}: {err}", path.display()),
            })?;
            removed.push(path.display().to_string());
        }
    }
    removed.sort();

    let body = GcBody {
        removed,
        warnings: inventory.warnings,
    };
    emit(Stream::Stdout, ctx.format, &body)?;
    if matches!(ctx.format, OutputFormat::Text) {
        emit_warnings_to_stderr(&body.warnings);
    }
    Ok(())
}

fn build_inventory(ctx: &Ctx) -> Result<Inventory> {
    let project_scope = ToolScope::Project {
        project_name: ctx.config.name.clone(),
    };
    validate_manifest_tools(&ctx.config.tools, &project_scope)?;
    let project_tools = load::project_tools(ctx.config.name.clone(), ctx.config.tools.clone());

    let mut scopes = vec![project_scope];
    let capability = resolve_project_capability(ctx)?;
    let capability_tools = if let Some(capability) = capability {
        let capability_scope = ToolScope::Capability {
            capability_slug: capability.manifest.name.clone(),
            capability_dir: capability.root_dir.clone(),
        };
        scopes.push(capability_scope.clone());
        let sidecar_tools =
            load::load_capability_sidecar(&capability.root_dir, &capability.manifest.name)?;
        let tools: Vec<Tool> = sidecar_tools.iter().map(|(_, tool)| tool.clone()).collect();
        validate_manifest_tools(&tools, &capability_scope)?;
        sidecar_tools
    } else {
        Vec::new()
    };

    let (merged, warnings) = load::merge_scoped(project_tools, capability_tools);
    Ok(Inventory {
        tools: merged.into_iter().map(|(scope, tool)| ScopedTool { scope, tool }).collect(),
        warnings: warnings.into_iter().map(warning_row).collect(),
        scopes,
    })
}

fn resolve_project_capability(ctx: &Ctx) -> Result<Option<ResolvedCapability>> {
    let Some(value) = ctx.config.capability.as_deref() else {
        return Ok(None);
    };
    let (root_dir, _) = Capability::locate(value, &ctx.project_dir)?;
    enforce_capability_filename(&root_dir)?;
    Capability::resolve(value, &ctx.project_dir).map(Some)
}

fn enforce_capability_filename(dir: &Path) -> Result<()> {
    Capability::probe_dir(dir).map(|_| ()).ok_or_else(|| Error::CapabilityManifestMissing {
        dir: dir.to_path_buf(),
    })
}

fn validate_manifest_tools(tools: &[Tool], scope: &ToolScope) -> Result<()> {
    let manifest = ToolManifest {
        tools: tools.to_vec(),
    };
    let summaries: Vec<ValidationSummary> =
        manifest.validate_structure(scope).iter().filter_map(validation_failure).collect();
    if summaries.is_empty() { Ok(()) } else { Err(Error::Validation { results: summaries }) }
}

fn validation_failure(result: &ToolValidationResult) -> Option<ValidationSummary> {
    match result {
        ToolValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => Some(ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: (*rule_id).to_string(),
            rule: (*rule).to_string(),
            detail: Some(detail.clone()),
        }),
        _ => None,
    }
}

fn warning_row(warning: Warning) -> WarningRow {
    match warning {
        Warning::ToolNameCollision { name } => WarningRow {
            code: "tool-name-collision",
            message: format!(
                "project-scope declaration for `{name}` overrides the capability-scope declaration"
            ),
            name,
        },
    }
}

fn find<'a>(inventory: &'a Inventory, name: &str) -> Result<&'a ScopedTool> {
    inventory.tools.iter().find(|scoped| scoped.tool.name == name).ok_or_else(|| {
        Error::ToolNotDeclared {
            name: name.to_string(),
        }
    })
}

fn select<'a>(inventory: &'a Inventory, name: Option<&str>) -> Result<Vec<&'a ScopedTool>> {
    match name {
        Some(name) => Ok(vec![find(inventory, name)?]),
        None => Ok(inventory.tools.iter().collect()),
    }
}

fn rows_for(tools: &[ScopedTool]) -> Result<Vec<ToolRow>> {
    tools.iter().map(row_for).collect()
}

fn row_for(scoped: &ScopedTool) -> Result<ToolRow> {
    let source = scoped.tool.source.to_wire_string().into_owned();
    let cache_status = cache_status_for(scoped)?;
    let cached_path = cache::module_path(&scoped.scope, &scoped.tool.name, &scoped.tool.version)?;
    let (scope, scope_detail) = scope_labels(&scoped.scope);
    Ok(ToolRow {
        name: scoped.tool.name.clone(),
        version: scoped.tool.version.clone(),
        source,
        scope,
        scope_detail,
        cache_status: cache_status_label(cache_status).to_string(),
        cached_path: cached_path.display().to_string(),
    })
}

fn show_row_for(scoped: &ScopedTool) -> Result<ToolShowRow> {
    let row = row_for(scoped)?;
    let sidecar_path = cache::sidecar_path(&scoped.scope, &scoped.tool.name, &scoped.tool.version)?;
    let fetched_at =
        cache::read_sidecar(&sidecar_path)?.map(|sidecar| sidecar.fetched_at.to_rfc3339());
    Ok(ToolShowRow {
        row,
        permissions: scoped.tool.permissions.clone(),
        sha256: scoped.tool.sha256.clone(),
        fetched_at,
    })
}

fn cache_status_for(scoped: &ScopedTool) -> Result<CacheStatus> {
    Ok(cache::cache_status(
        &scoped.scope,
        &scoped.tool.name,
        &scoped.tool.version,
        scoped.tool.source.to_wire_string().as_ref(),
        scoped.tool.sha256.as_deref(),
    )?)
}

const fn cache_status_label(status: CacheStatus) -> &'static str {
    match status {
        CacheStatus::Hit => "hit",
        CacheStatus::MissNotFound => "miss-not-found",
        CacheStatus::MissChanged => "miss-changed",
    }
}

fn scope_labels(scope: &ToolScope) -> (String, String) {
    match scope {
        ToolScope::Project { project_name } => ("project".to_string(), project_name.clone()),
        ToolScope::Capability { capability_slug, .. } => {
            ("capability".to_string(), capability_slug.clone())
        }
    }
}

fn kept_by_scope(inventory: &Inventory) -> HashMap<ToolScope, HashSet<CacheKey>> {
    let mut kept: HashMap<ToolScope, HashSet<CacheKey>> =
        inventory.scopes.iter().cloned().map(|scope| (scope, HashSet::new())).collect();
    for scoped in &inventory.tools {
        kept.entry(scoped.scope.clone()).or_default().insert((
            scoped.tool.name.clone(),
            scoped.tool.version.clone(),
            scoped.tool.source.to_wire_string().into_owned(),
        ));
    }
    kept
}

fn emit_warnings_to_stderr(warnings: &[WarningRow]) {
    for warning in warnings {
        eprintln!("warning: {}: {}", warning.code, warning.message);
    }
}

fn format_permission_list(values: &[String]) -> String {
    if values.is_empty() { "(none)".to_string() } else { values.join(", ") }
}
