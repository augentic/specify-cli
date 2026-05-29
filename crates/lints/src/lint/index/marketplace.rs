//! `.cursor-plugin/marketplace.json` extractor per the standards-layer
//! contract §"Module additions".
//!
//! Emits one [`MarketplaceEntry`] fact per element of the
//! `plugins[]` array under the marketplace manifest. The
//! `path-in-manifest` field is a JSON-pointer-style location
//! (e.g. `/plugins/0`) so downstream rules can reason about manifest
//! offsets without re-reading the file. Parse failures, missing
//! `plugins` arrays, and per-entry `name` lookups that fail to
//! produce a non-empty string collapse to a silent skip — the file
//! scan contract reserves the `index.warning` finding for the hint
//! runner.

use serde_json::Value;

use super::files::DiscoveredFile;
use crate::lint::MarketplaceEntry;

const MARKETPLACE_RELATIVE: &str = ".cursor-plugin/marketplace.json";

/// Extract [`MarketplaceEntry`] facts from a discovered file.
///
/// Returns an empty vector for any file that is not the canonical
/// `.cursor-plugin/marketplace.json` and for files whose JSON body
/// does not parse as an object carrying a `plugins[]` array of
/// objects with non-empty `name` fields.
#[must_use]
pub fn extract(file: &DiscoveredFile) -> Vec<MarketplaceEntry> {
    if file.relative != MARKETPLACE_RELATIVE {
        return Vec::new();
    }
    let text = file.text();
    if text.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(plugins) = value.get("plugins").and_then(Value::as_array) else {
        return Vec::new();
    };
    plugins
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let name = entry.get("name").and_then(Value::as_str)?.trim();
            if name.is_empty() {
                return None;
            }
            Some(MarketplaceEntry {
                plugin: name.to_owned(),
                path_in_manifest: format!("/plugins/{idx}"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::FileKind;

    fn marketplace(relative: &str, body: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: relative.into(),
            kind: FileKind::Text,
            language: Some("json".into()),
            bytes: Some(body.as_bytes().to_vec()),
        }
    }

    #[test]
    fn extracts_each_plugin_entry() {
        let file = marketplace(
            MARKETPLACE_RELATIVE,
            r#"{"plugins":[{"name":"spec"},{"name":"capture"}]}"#,
        );
        let entries = extract(&file);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].plugin, "spec");
        assert_eq!(entries[0].path_in_manifest, "/plugins/0");
        assert_eq!(entries[1].plugin, "capture");
        assert_eq!(entries[1].path_in_manifest, "/plugins/1");
    }

    #[test]
    fn rejects_non_marketplace_path() {
        let file = marketplace("plugins/marketplace.json", r#"{"plugins":[{"name":"x"}]}"#);
        assert!(extract(&file).is_empty());
    }

    #[test]
    fn returns_empty_for_missing_plugins_array() {
        let file = marketplace(MARKETPLACE_RELATIVE, r#"{"name":"augentic"}"#);
        assert!(extract(&file).is_empty());
    }
}
