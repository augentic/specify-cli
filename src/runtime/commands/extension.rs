//! `specify tool *` dispatcher. Hosts the shared inventory-assembly
//! helpers (declared-tool merge, adapter resolution, manifest
//! validation) consumed by every per-subcommand handler.

pub mod cli;
mod dto;
mod fetch;
mod gc;
mod run;

use std::collections::{HashMap, HashSet};

pub(super) use fetch::run as fetch;
pub(super) use gc::run as gc;
pub(super) use run::{run, run_captured};
use specify_error::{Error, Result};
use specify_registry::load::{self};
use specify_registry::manifest::{Axis as ToolAxis, Extension, ExtensionManifest, ExtensionScope};
use specify_workflow::adapter::{ResolvedTargetAdapter, TargetAdapter};
use specify_workflow::init::adapter_ref_from_value;

pub(super) use self::dto::{Inventory, ScopedTool};
use self::dto::{WarningRow, warning_row};
use crate::runtime::context::Ctx;

pub fn build_inventory(ctx: &Ctx) -> Result<Inventory> {
    let project_scope = ExtensionScope::Project {
        project_name: ctx.config.name.clone(),
    };
    validate_manifest_tools(&ctx.config.tools, &project_scope)?;
    let project_tools = load::project_tools(ctx.config.name.clone(), ctx.config.tools.clone());

    let mut scopes = vec![project_scope];
    let plugin = resolve_project_adapter(ctx)?;
    let plugin_tools = if let Some(plugin) = plugin {
        let plugin_scope = ExtensionScope::Plugin {
            axis: ToolAxis::Target,
            plugin_slug: plugin.manifest.name.clone(),
            capability_dir: plugin.location.path().clone(),
        };
        scopes.push(plugin_scope.clone());
        let sidecar_tools =
            load::plugin_sidecar(plugin.location.path(), &plugin.manifest.name, ToolAxis::Target)?;
        let tools: Vec<Extension> = sidecar_tools.iter().map(|(_, tool)| tool.clone()).collect();
        validate_manifest_tools(&tools, &plugin_scope)?;
        sidecar_tools
    } else {
        Vec::new()
    };

    let (merged, warnings) = load::merge_scoped(project_tools, plugin_tools);
    Ok(Inventory {
        tools: merged.into_iter().map(|(scope, tool)| ScopedTool { scope, tool }).collect(),
        warnings: warnings.into_iter().map(warning_row).collect(),
        scopes,
    })
}

fn resolve_project_adapter(ctx: &Ctx) -> Result<Option<ResolvedTargetAdapter>> {
    let Some(value) = ctx.config.adapter.as_deref() else {
        return Ok(None);
    };
    let adapter_ref = adapter_ref_from_value(value);
    TargetAdapter::resolve(&adapter_ref, &ctx.project_dir).map(Some)
}

fn validate_manifest_tools(tools: &[Extension], scope: &ExtensionScope) -> Result<()> {
    let manifest = ExtensionManifest {
        tools: tools.to_vec(),
    };
    // `validate_structure` returns one deterministic `violation`
    // diagnostic per failing rule (passing rules emit nothing), so an
    // empty vector means the manifest is structurally valid. Collapse
    // any failures into a single payload-free `Error::Validation` keyed
    // on the first rule id; per-row detail is joined into the message.
    let diagnostics = manifest.validate_structure(scope);
    let Some(first) = diagnostics.first() else {
        return Ok(());
    };
    let code = first.rule_id.clone().unwrap_or_else(|| "tool-manifest-invalid".to_string());
    let detail = diagnostics.iter().map(|d| d.impact.as_str()).collect::<Vec<_>>().join("; ");
    Err(Error::validation_failed(code, "tools.yaml manifest must satisfy structural rules", detail))
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

fn kept_by_scope(
    inventory: &Inventory,
) -> HashMap<ExtensionScope, HashSet<(String, String, String)>> {
    let mut kept: HashMap<ExtensionScope, HashSet<(String, String, String)>> =
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
