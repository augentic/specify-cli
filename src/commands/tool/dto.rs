//! Response DTOs and row builders for `specify tool *` handlers.

use std::io::Write;

use serde::Serialize;
use specify_error::Result;
use specify_tool::cache::{self, OciSnapshot, PackageSnapshot, Status as CacheStatus};
use specify_tool::load::Collision;
use specify_tool::{Tool, ToolPermissions, ToolScope, ToolScopeKind};

pub(super) type CacheKey = (String, String, String);

#[derive(Debug, Clone)]
pub(super) struct ScopedTool {
    pub(super) scope: ToolScope,
    pub(super) tool: Tool,
}

#[derive(Debug)]
pub(super) struct Inventory {
    pub(super) tools: Vec<ScopedTool>,
    pub(super) warnings: Vec<WarningRow>,
    pub(super) scopes: Vec<ToolScope>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct WarningRow {
    pub(super) code: &'static str,
    pub(super) name: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ToolRow {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) source: String,
    pub(super) scope: ToolScopeKind,
    pub(super) scope_detail: String,
    pub(super) cache_status: CacheStatus,
    pub(super) cached_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ToolFetchRow {
    #[serde(flatten)]
    pub(super) row: ToolRow,
    pub(super) fetched: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ToolShowRow {
    #[serde(flatten)]
    pub(super) row: ToolRow,
    pub(super) permissions: ToolPermissions,
    pub(super) sha256: Option<String>,
    pub(super) fetched_at: Option<String>,
    pub(super) package: Option<PackageSnapshot>,
    pub(super) oci: Option<OciSnapshot>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ListBody {
    pub(super) tools: Vec<ToolRow>,
    pub(super) warnings: Vec<WarningRow>,
}

pub(super) fn write_list_text(w: &mut dyn Write, body: &ListBody) -> std::io::Result<()> {
    if body.tools.is_empty() {
        writeln!(w, "No declared tools.")?;
        return Ok(());
    }
    writeln!(w, "name\tversion\tscope\tcache\tcached path")?;
    for row in &body.tools {
        writeln!(
            w,
            "{}\t{}\t{}:{}\t{}\t{}",
            row.name, row.version, row.scope, row.scope_detail, row.cache_status, row.cached_path
        )?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct FetchBody {
    pub(super) tools: Vec<ToolFetchRow>,
    pub(super) warnings: Vec<WarningRow>,
}

pub(super) fn write_fetch_text(w: &mut dyn Write, body: &FetchBody) -> std::io::Result<()> {
    if body.tools.is_empty() {
        writeln!(w, "No declared tools to fetch.")?;
        return Ok(());
    }
    for row in &body.tools {
        let action = if row.fetched { "fetched" } else { "cached" };
        writeln!(
            w,
            "{action}: {} {} [{}:{}] {}",
            row.row.name, row.row.version, row.row.scope, row.row.scope_detail, row.row.cached_path
        )?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ShowBody {
    pub(super) tool: ToolShowRow,
    pub(super) warnings: Vec<WarningRow>,
}

pub(super) fn write_show_text(w: &mut dyn Write, body: &ShowBody) -> std::io::Result<()> {
    let row = &body.tool;
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
    if let Some(package) = &row.package {
        writeln!(w, "package: {}@{} ({})", package.name, package.version, package.registry)?;
    }
    if let Some(oci) = &row.oci {
        writeln!(w, "oci: {}", oci.reference)?;
    }
    writeln!(w, "permissions:")?;
    writeln!(w, "  read: {}", format_permission_list(&row.permissions.read))?;
    writeln!(w, "  write: {}", format_permission_list(&row.permissions.write))?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct GcBody {
    pub(super) removed: Vec<String>,
    pub(super) warnings: Vec<WarningRow>,
}

pub(super) fn write_gc_text(w: &mut dyn Write, body: &GcBody) -> std::io::Result<()> {
    writeln!(
        w,
        "Removed {} tool cache entrie(s) from current-project scopes.",
        body.removed.len()
    )?;
    for path in &body.removed {
        writeln!(w, "  {path}")?;
    }
    Ok(())
}

pub(super) fn rows_for(tools: &[ScopedTool]) -> Result<Vec<ToolRow>> {
    tools.iter().map(row_for).collect()
}

pub(super) fn row_for(scoped: &ScopedTool) -> Result<ToolRow> {
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
        cache_status,
        cached_path: cached_path.display().to_string(),
    })
}

pub(super) fn show_row_for(scoped: &ScopedTool) -> Result<ToolShowRow> {
    let row = row_for(scoped)?;
    let sidecar_path = cache::sidecar_path(&scoped.scope, &scoped.tool.name, &scoped.tool.version)?;
    let sidecar = cache::read_sidecar(&sidecar_path)?;
    let fetched_at = sidecar
        .as_ref()
        .map(|sidecar| sidecar.fetched_at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string());
    let package = sidecar.as_ref().and_then(|sidecar| sidecar.package.clone());
    let oci = sidecar.as_ref().and_then(|sidecar| sidecar.oci.clone());
    Ok(ToolShowRow {
        row,
        permissions: scoped.tool.permissions.clone(),
        sha256: scoped.tool.sha256.clone(),
        fetched_at,
        package,
        oci,
    })
}

pub(super) fn cache_status_for(scoped: &ScopedTool) -> Result<CacheStatus> {
    Ok(cache::status(
        &scoped.scope,
        &scoped.tool.name,
        &scoped.tool.version,
        scoped.tool.source.to_wire_string().as_ref(),
        scoped.tool.sha256.as_deref(),
    )?)
}

pub(super) fn scope_labels(scope: &ToolScope) -> (ToolScopeKind, String) {
    match scope {
        ToolScope::Project { project_name } => (ToolScopeKind::Project, project_name.clone()),
        ToolScope::Capability { capability_slug, .. } => {
            (ToolScopeKind::Capability, capability_slug.clone())
        }
    }
}

pub(super) fn warning_row(collision: Collision) -> WarningRow {
    let Collision { name } = collision;
    WarningRow {
        code: "tool-name-collision",
        message: format!(
            "project-scope declaration for `{name}` overrides the capability-scope declaration"
        ),
        name,
    }
}

pub(super) fn format_permission_list(values: &[String]) -> String {
    if values.is_empty() { "(none)".to_string() } else { values.join(", ") }
}
