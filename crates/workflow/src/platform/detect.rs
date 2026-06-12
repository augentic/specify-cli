//! Host-side wrapper around `vectis verify --mode detect` for Vectis-bound
//! projects. Non-Vectis targets return an empty missing set without
//! dispatching the tool (RFC-46 Phase 0).

#[cfg(test)]
#[path = "detect/tests.rs"]
mod tests;

use std::collections::HashSet;
use std::path::Path;

use jiff::Timestamp;
use serde_json::Value;
use specify_error::{Error, Result};
use specify_tool::host::{RunContext, WasiRunner};
use specify_tool::load;
use specify_tool::manifest::{Axis as ToolAxis, Tool, ToolScope};

use crate::adapter::TargetAdapter;
use crate::config::ProjectConfig;
use crate::init::adapter_name_from_value;
use crate::Platform;

const VECTIS_ADAPTER: &str = "vectis";
const VECTIS_TOOL: &str = "vectis";

/// Return declared-but-absent platforms for a Vectis-bound project by
/// dispatching `vectis verify --mode detect`.
///
/// When the project's target adapter is not [`VECTIS_ADAPTER`], returns
/// an empty vector without invoking the tool (Omnia and other targets are
/// unaffected). The result preserves `declared` order and is filtered to
/// platforms present in both the caller's `declared` set and the tool's
/// `missing[]` response.
///
/// # Errors
///
/// - Propagates [`ProjectConfig::load`] failures.
/// - `tool-not-declared` when the vectis tool is absent from merged
///   declarations.
/// - `tool-runtime` / resolver errors from Wasmtime dispatch.
/// - `vectis-detect-parse` when stdout is not the expected detect JSON.
/// - `vectis-detect-failed` when the tool exits non-zero.
pub fn vectis_missing_platforms(
    project_dir: &Path, declared: &[Platform],
) -> Result<Vec<Platform>, Error> {
    if declared.is_empty() || !is_vectis_bound(project_dir)? {
        return Ok(Vec::new());
    }

    let missing = dispatch_vectis_detect(project_dir)?;
    let missing_set: HashSet<Platform> = missing.into_iter().collect();
    Ok(declared.iter().copied().filter(|p| missing_set.contains(p)).collect())
}

fn is_vectis_bound(project_dir: &Path) -> Result<bool, Error> {
    let config = ProjectConfig::load(project_dir)?;
    let Some(adapter_value) = config.adapter.as_deref() else {
        return Ok(false);
    };
    let name = adapter_name_from_value(adapter_value);
    let resolved = TargetAdapter::resolve(name, project_dir)?;
    Ok(resolved.manifest.name == VECTIS_ADAPTER)
}

fn dispatch_vectis_detect(project_dir: &Path) -> Result<Vec<Platform>, Error> {
    let config = ProjectConfig::load(project_dir)?;
    let inventory = merged_tool_inventory(project_dir, &config)?;
    let (scope, tool) = find_vectis_tool(&inventory)?;

    let resolved =
        specify_tool::resolver::resolve(scope, tool, Timestamp::now(), project_dir)?;
    let args = vec![
        "verify".to_string(),
        "--mode".to_string(),
        "detect".to_string(),
    ];
    let mut run_ctx = RunContext::new(project_dir, args);
    if let ToolScope::Plugin { capability_dir, .. } = scope {
        run_ctx = run_ctx.with_capability_dir(capability_dir);
    }

    let runner = WasiRunner::new()?;
    let captured = runner.run_captured(&resolved, &run_ctx).map_err(Error::from)?;

    if captured.exit_code != 0 {
        let stderr = String::from_utf8_lossy(&captured.stderr);
        let stdout = String::from_utf8_lossy(&captured.stdout);
        return Err(Error::Diag {
            code: "vectis-detect-failed",
            detail: format!(
                "vectis verify --mode detect exited with code {}: {}",
                captured.exit_code,
                if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() }
            ),
        });
    }

    parse_detect_missing(&captured.stdout)
}

fn merged_tool_inventory(
    project_dir: &Path, config: &ProjectConfig,
) -> Result<Vec<(ToolScope, Tool)>, Error> {
    let project_tools = load::project_tools(config.name.clone(), config.tools.clone());
    let plugin_tools = if let Some(adapter_value) = config.adapter.as_deref() {
        let name = adapter_name_from_value(adapter_value);
        let resolved = TargetAdapter::resolve(name, project_dir)?;
        load::plugin_sidecar(
            resolved.location.path(),
            &resolved.manifest.name,
            ToolAxis::Target,
        )
        .map_err(Error::from)?
    } else {
        Vec::new()
    };
    let (merged, _warnings) = load::merge_scoped(project_tools, plugin_tools);
    Ok(merged)
}

fn find_vectis_tool(inventory: &[(ToolScope, Tool)]) -> Result<(&ToolScope, &Tool), Error> {
    inventory
        .iter()
        .find(|(_, tool)| tool.name == VECTIS_TOOL)
        .map(|(scope, tool)| (scope, tool))
        .ok_or_else(|| {
            Error::validation_failed(
                "tool-not-declared",
                "tool must be declared in tools.yaml",
                format!("tool not declared: {VECTIS_TOOL}"),
            )
        })
}

fn parse_detect_missing(stdout: &[u8]) -> Result<Vec<Platform>, Error> {
    let value: Value = serde_json::from_slice(stdout).map_err(|err| Error::Diag {
        code: "vectis-detect-parse",
        detail: format!("vectis detect output is not valid JSON: {err}"),
    })?;

    if let Some(error) = value.get("error") {
        return Err(Error::Diag {
            code: "vectis-detect-failed",
            detail: format!("vectis detect returned error payload: {error}"),
        });
    }

    let missing = value.get("missing").and_then(Value::as_array).ok_or_else(|| Error::Diag {
        code: "vectis-detect-parse",
        detail: "vectis detect response missing `missing` array".to_string(),
    })?;

    missing.iter().map(parse_missing_entry).collect()
}

fn parse_missing_entry(entry: &Value) -> Result<Platform, Error> {
    let name = entry.as_str().ok_or_else(|| Error::Diag {
        code: "vectis-detect-parse",
        detail: "vectis detect `missing` entry is not a string".to_string(),
    })?;
    name.parse::<Platform>().map_err(|err| Error::Diag {
        code: "vectis-detect-parse",
        detail: format!("vectis detect returned unknown platform `{name}`: {err}"),
    })
}
