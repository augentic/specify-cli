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
use crate::lint::{AdapterAxis, AdapterManifest};

#[derive(Debug, Deserialize)]
struct ManifestBody {
    name: Option<String>,
    version: Option<serde_json::Value>,
    briefs: Option<BTreeMap<String, serde_json::Value>>,
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
    Some(AdapterManifest {
        axis,
        name: name.to_owned(),
        path: file.relative.clone(),
        version,
        brief_keys,
    })
}

/// Split `adapters/{sources,targets}/<adapter>/adapter.yaml` into the
/// `(axis, adapter)` pair. Returns `None` for any other shape so the
/// extractor never confuses a nested `adapter.yaml` for a top-level
/// adapter manifest.
fn parse_manifest_path(relative: &str) -> Option<(AdapterAxis, &str)> {
    let rest = relative.strip_prefix("adapters/")?;
    let (axis_str, rest) = rest.split_once('/')?;
    let axis = match axis_str {
        "sources" => AdapterAxis::Sources,
        "targets" => AdapterAxis::Targets,
        _ => return None,
    };
    let (adapter, tail) = rest.split_once('/')?;
    if adapter.is_empty() || tail != "adapter.yaml" {
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
mod tests {
    use super::*;
    use crate::lint::FileKind;

    fn manifest(relative: &str, body: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: relative.into(),
            kind: FileKind::Text,
            language: Some("yaml".into()),
            bytes: Some(body.as_bytes().to_vec()),
        }
    }

    #[test]
    fn extracts_source_manifest() {
        let file = manifest(
            "adapters/sources/intent/adapter.yaml",
            "name: intent\nversion: 1\naxis: source\n",
        );
        let manifest = extract(&file).expect("manifest extracted");
        assert_eq!(manifest.axis, AdapterAxis::Sources);
        assert_eq!(manifest.name, "intent");
        assert_eq!(manifest.version.as_deref(), Some("1"));
        assert!(manifest.brief_keys.is_empty());
    }

    #[test]
    fn extracts_target_manifest_string_version() {
        let file =
            manifest("adapters/targets/omnia/adapter.yaml", "name: omnia\nversion: \"2.1\"\n");
        let manifest = extract(&file).expect("manifest extracted");
        assert_eq!(manifest.axis, AdapterAxis::Targets);
        assert_eq!(manifest.version.as_deref(), Some("2.1"));
    }

    #[test]
    fn extracts_briefs_keys_when_declared() {
        let file = manifest(
            "adapters/sources/intent/adapter.yaml",
            "name: intent\nversion: 1\nbriefs:\n  enumerate: briefs/enumerate.md\n  extract: briefs/extract.md\n",
        );
        let manifest = extract(&file).expect("manifest extracted");
        assert_eq!(manifest.brief_keys, vec!["enumerate".to_string(), "extract".to_string()]);
    }

    #[test]
    fn missing_briefs_map_leaves_brief_keys_empty() {
        let file = manifest("adapters/targets/omnia/adapter.yaml", "name: omnia\nversion: 1\n");
        let manifest = extract(&file).expect("manifest extracted");
        assert!(manifest.brief_keys.is_empty());
    }

    #[test]
    fn rejects_unknown_axis() {
        let file = manifest("adapters/whatever/intent/adapter.yaml", "name: intent\n");
        assert!(extract(&file).is_none());
    }

    #[test]
    fn rejects_nested_adapter_yaml() {
        let file = manifest("adapters/sources/intent/briefs/adapter.yaml", "name: nope\n");
        assert!(extract(&file).is_none());
    }
}
