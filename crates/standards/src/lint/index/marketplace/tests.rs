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
    let file =
        marketplace(MARKETPLACE_RELATIVE, r#"{"plugins":[{"name":"spec"},{"name":"capture"}]}"#);
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
