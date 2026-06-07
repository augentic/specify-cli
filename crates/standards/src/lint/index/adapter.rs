//! `adapters/**/adapter.yaml` extractor per the standards-layer
//! contract §"Module additions".
//!
//! Emits one [`AdapterManifest`] fact per `adapter.yaml` whose
//! project-relative path matches
//! `adapters/{sources,targets}/<name>/adapter.yaml`. The body is
//! parsed via `serde_saphyr` into a tolerant DTO that only requires
//! the `name:` field; the optional `version:` field is forwarded
//! when present so consumer rules can pin manifest versions without
//! re-reading the YAML. Files outside the canonical layout, or files
//! whose YAML body fails to parse as an object, collapse to a silent
//! per-file skip — the file scan contract reserves the `index.warning`
//! finding for the hint runner.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::files::DiscoveredFile;
use crate::lint::{AdapterAxis, AdapterManifest, AdapterTool};

#[derive(Debug, Deserialize)]
struct ManifestBody {
    name: Option<String>,
    version: Option<serde_json::Value>,
    briefs: Option<BTreeMap<String, serde_json::Value>>,
    /// Parsed tolerantly as opaque values so a malformed entry never
    /// drops the whole manifest fact; well-formed `{ name, version }`
    /// pairs are projected in `collect_tools`, malformed ones skipped
    /// (their shape is the `adapter` registered schema's concern).
    tools: Option<Vec<serde_json::Value>>,
}

/// Extract an [`AdapterManifest`] fact from a discovered file.
///
/// Returns `None` for files that do not live under
/// `adapters/{sources,targets}/<name>/adapter.yaml`, for binary
/// `adapter.yaml` files, and for YAML bodies that fail to parse as a
/// mapping carrying a non-empty `name:` value.
#[must_use]
pub fn extract(file: &DiscoveredFile) -> Option<AdapterManifest> {
    let (axis, _adapter) = parse_manifest_path(&file.relative)?;
    let text = file.text();
    if text.is_empty() {
        return None;
    }
    let body: ManifestBody = serde_saphyr::from_str(&text).ok()?;
    let name = body.name?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let version = body.version.and_then(stringify_version);
    let brief_keys = body.briefs.map(|map| map.into_keys().collect::<Vec<_>>()).unwrap_or_default();
    let tools = collect_tools(body.tools);
    Some(AdapterManifest {
        axis,
        name: name.to_owned(),
        path: file.relative.clone(),
        version,
        brief_keys,
        tools,
    })
}

/// Project the tolerant `tools[]` value list into [`AdapterTool`] facts,
/// keeping only entries that carry both a string `name` and a string
/// `version`. Malformed entries are dropped silently — the `adapter`
/// registered schema (CORE-001) owns the shape violation.
fn collect_tools(tools: Option<Vec<serde_json::Value>>) -> Vec<AdapterTool> {
    let Some(tools) = tools else {
        return Vec::new();
    };
    tools
        .into_iter()
        .filter_map(|tool| {
            let object = tool.as_object()?;
            let name = object.get("name").and_then(serde_json::Value::as_str)?;
            let version = object.get("version").and_then(serde_json::Value::as_str)?;
            Some(AdapterTool {
                name: name.to_owned(),
                version: version.to_owned(),
            })
        })
        .collect()
}

/// Split `adapters/{sources,targets}/<adapter>/adapter.yaml` into the
/// `(axis, adapter)` pair. Returns `None` for any other shape so the
/// extractor never confuses a nested `adapter.yaml` for a top-level
/// adapter manifest.
fn parse_manifest_path(relative: &str) -> Option<(AdapterAxis, &str)> {
    let (axis, adapter, tail) = super::path_util::parse_adapter_prefix(relative)?;
    if tail != "adapter.yaml" {
        return None;
    }
    Some((axis, adapter))
}

/// `version:` is permitted as an integer or string by the on-disk
/// manifests; flatten both forms to the canonical string the
/// `WorkspaceModel` carries.
fn stringify_version(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) }
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
