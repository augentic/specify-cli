//! Response DTOs and row builders for `specrun tool *` handlers.

use std::io::Write;

use serde::Serialize;
use specify_error::Result;
use specify_tool::cache::{self, Status as CacheStatus};
use specify_tool::manifest::{Tool, ToolScope, ToolScopeKind};

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

pub(super) fn cache_status_for(scoped: &ScopedTool) -> Result<CacheStatus> {
    Ok(cache::status(
        &scoped.scope,
        &scoped.tool.name,
        &scoped.tool.version,
        scoped.tool.source.to_wire_string().as_ref(),
        scoped.tool.sha256.as_deref(),
    )?)
}

fn scope_labels(scope: &ToolScope) -> (ToolScopeKind, String) {
    match scope {
        ToolScope::Project { project_name } => (ToolScopeKind::Project, project_name.clone()),
        ToolScope::Plugin { plugin_slug, .. } => (ToolScopeKind::Plugin, plugin_slug.clone()),
    }
}

pub(super) fn warning_row(name: String) -> WarningRow {
    WarningRow {
        code: "tool-name-collision",
        message: format!(
            "project-scope declaration for `{name}` overrides the plugin-scope declaration"
        ),
        name,
    }
}
