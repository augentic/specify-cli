//! `specify tool *` dispatcher. Hosts the shared inventory-assembly
//! helpers (declared-tool merge, adapter resolution, manifest
//! validation) consumed by every per-subcommand handler.

pub mod cli;
mod dto;
mod fetch;
mod gc;
mod list;
mod run;
mod show;

use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(super) use fetch::run as fetch;
pub(super) use gc::run as gc;
pub(super) use list::run as list;
pub(super) use run::run;
pub(super) use show::run as show;
use specify_domain::adapter::{Adapter, ResolvedAdapter};
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};
use specify_tool::load::{self};
use specify_tool::manifest::{Tool, ToolManifest, ToolScope};

use self::dto::{CacheKey, Inventory, ScopedTool, WarningRow, warning_row};
use crate::context::Ctx;

fn build_inventory(ctx: &Ctx) -> Result<Inventory> {
    let project_scope = ToolScope::Project {
        project_name: ctx.config.name.clone(),
    };
    validate_manifest_tools(&ctx.config.tools, &project_scope)?;
    let project_tools = load::project_tools(ctx.config.name.clone(), ctx.config.tools.clone());

    let mut scopes = vec![project_scope];
    let adapter = resolve_project_adapter(ctx)?;
    let adapter_tools = if let Some(adapter) = adapter {
        let adapter_scope = ToolScope::Adapter {
            adapter_slug: adapter.manifest.name.clone(),
            adapter_dir: adapter.root_dir.clone(),
        };
        scopes.push(adapter_scope.clone());
        let sidecar_tools = load::adapter_sidecar(&adapter.root_dir, &adapter.manifest.name)?;
        let tools: Vec<Tool> = sidecar_tools.iter().map(|(_, tool)| tool.clone()).collect();
        validate_manifest_tools(&tools, &adapter_scope)?;
        sidecar_tools
    } else {
        Vec::new()
    };

    let (merged, warnings) = load::merge_scoped(project_tools, adapter_tools);
    Ok(Inventory {
        tools: merged.into_iter().map(|(scope, tool)| ScopedTool { scope, tool }).collect(),
        warnings: warnings.into_iter().map(warning_row).collect(),
        scopes,
    })
}

fn resolve_project_adapter(ctx: &Ctx) -> Result<Option<ResolvedAdapter>> {
    let Some(value) = ctx.config.adapter.as_deref() else {
        return Ok(None);
    };
    let (root_dir, _) = Adapter::locate(value, &ctx.project_dir)?;
    enforce_adapter_filename(&root_dir)?;
    Adapter::resolve(value, &ctx.project_dir).map(Some)
}

fn enforce_adapter_filename(dir: &Path) -> Result<()> {
    Adapter::probe_dir(dir).map(|_| ()).ok_or_else(|| Error::Diag {
        code: "adapter-manifest-missing",
        detail: format!("no `adapter.yaml` at {}", dir.display()),
    })
}

fn validate_manifest_tools(tools: &[Tool], scope: &ToolScope) -> Result<()> {
    let manifest = ToolManifest {
        tools: tools.to_vec(),
    };
    let summaries: Vec<ValidationSummary> = manifest
        .validate_structure(scope)
        .into_iter()
        .filter(|summary| summary.status == ValidationStatus::Fail)
        .collect();
    if summaries.is_empty() { Ok(()) } else { Err(Error::Validation { results: summaries }) }
}

fn find<'a>(inventory: &'a Inventory, name: &str) -> Result<&'a ScopedTool> {
    inventory.tools.iter().find(|scoped| scoped.tool.name == name).ok_or_else(|| {
        Error::validation_failed(
            "tool-not-declared",
            "tool must be declared in tools.yaml",
            format!("tool not declared: {name}"),
        )
    })
}

fn select<'a>(inventory: &'a Inventory, name: Option<&str>) -> Result<Vec<&'a ScopedTool>> {
    match name {
        Some(name) => Ok(vec![find(inventory, name)?]),
        None => Ok(inventory.tools.iter().collect()),
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
