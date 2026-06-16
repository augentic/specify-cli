//! Auto-write absent `sources.<platform>` pins after materialize (RFC-46 Resolved §7).

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value};

use crate::materialize::paths::{Platform, export_layout};
use crate::VectisError;

/// One platform slot whose export was written from `source:` in this invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoPin {
    pub asset_id: String,
    pub platform: String,
    pub path: String,
}

/// Derive canonical pin paths for platform slots materialized in this run.
#[must_use]
pub fn collect_auto_pins(materialized: &[Value], assets: &Map<String, Value>) -> Vec<AutoPin> {
    let mut seen = HashSet::new();
    let mut pins = Vec::new();

    for entry in materialized {
        let Some(asset_id) = entry.get("asset_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(platform_name) = entry.get("platform").and_then(Value::as_str) else {
            continue;
        };
        let key = (asset_id.to_string(), platform_name.to_string());
        if !seen.insert(key) {
            continue;
        }

        let Some(asset_entry) = assets.get(asset_id) else {
            continue;
        };
        let role = asset_entry.get("role").and_then(Value::as_str).unwrap_or("icon");
        let kind = asset_entry.get("kind").and_then(Value::as_str).unwrap_or("vector");
        let Some(platform) = Platform::parse(platform_name) else {
            continue;
        };
        let Some(layout) = export_layout(role, kind, platform, asset_id) else {
            continue;
        };

        pins.push(AutoPin {
            asset_id: asset_id.to_string(),
            platform: platform_name.to_string(),
            path: layout.pin,
        });
    }

    pins
}

/// Merge auto-written pins into the in-memory `assets.yaml` document.
///
/// Existing pins are never overwritten (Resolved §6).
pub fn apply_auto_pins(instance: &mut Value, pins: &[AutoPin]) {
    let Some(assets) = instance.get_mut("assets").and_then(Value::as_object_mut) else {
        return;
    };

    for pin in pins {
        let Some(entry) = assets.get_mut(&pin.asset_id).and_then(Value::as_object_mut) else {
            continue;
        };
        let sources = entry
            .entry("sources")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(sources_obj) = sources.as_object_mut()
            && !sources_obj.contains_key(&pin.platform)
        {
            sources_obj.insert(pin.platform.clone(), Value::String(pin.path.clone()));
        }
    }
}

/// Serialise `instance` as YAML with a guaranteed trailing newline.
///
/// # Errors
///
/// Returns [`VectisError::Internal`] when serialisation fails.
pub fn serialise_yaml(instance: &Value) -> Result<String, VectisError> {
    let mut content = serde_saphyr::to_string(instance).map_err(|err| VectisError::Internal {
        message: format!("assets.yaml serialise failed: {err}"),
    })?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    Ok(content)
}

/// Atomically persist YAML at `path` (temp file in the same parent, then rename).
///
/// # Errors
///
/// Returns [`VectisError::Io`] or [`VectisError::InvalidProject`] on write failure.
pub fn atomic_yaml_write(path: &Path, content: &str) -> Result<(), VectisError> {
    atomic_bytes_write(path, content.as_bytes())
}

fn atomic_bytes_write(path: &Path, bytes: &[u8]) -> Result<(), VectisError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path.file_name().ok_or_else(|| VectisError::InvalidProject {
        message: format!("cannot write assets.yaml: invalid path {}", path.display()),
    })?;
    let tmp_path = parent.join(format!(".{}.tmp", file_name.to_string_lossy()));
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_auto_pins_dedupes_artifact_entries() {
        let assets = Map::from_iter([(
            "settings".to_string(),
            json!({
                "kind": "vector",
                "role": "icon",
                "source": "assets/settings.svg",
            }),
        )]);
        let materialized = vec![
            json!({
                "asset_id": "settings",
                "platform": "ios",
                "path": "assets/exports/ios/settings.imageset/settings.pdf",
            }),
            json!({
                "asset_id": "settings",
                "platform": "ios",
                "path": "assets/exports/ios/settings.imageset/Contents.json",
            }),
        ];

        let pins = collect_auto_pins(&materialized, &assets);
        assert_eq!(pins.len(), 1);
        assert_eq!(
            pins[0].path,
            "assets/exports/ios/settings.imageset/settings.pdf"
        );
    }

    #[test]
    fn apply_auto_pins_fills_absent_platform_slots_only() {
        let mut instance = json!({
            "version": 1,
            "assets": {
                "settings": {
                    "kind": "vector",
                    "role": "icon",
                    "source": "assets/settings.svg",
                    "sources": {
                        "ios": "assets/exports/ios/settings.imageset/settings.pdf"
                    }
                }
            }
        });
        let pins = vec![
            AutoPin {
                asset_id: "settings".into(),
                platform: "ios".into(),
                path: "assets/exports/ios/settings.imageset/settings.pdf".into(),
            },
            AutoPin {
                asset_id: "settings".into(),
                platform: "android".into(),
                path: "assets/exports/android/drawable/settings.xml".into(),
            },
        ];

        apply_auto_pins(&mut instance, &pins);

        let sources = &instance["assets"]["settings"]["sources"];
        assert_eq!(
            sources["ios"],
            "assets/exports/ios/settings.imageset/settings.pdf"
        );
        assert_eq!(sources["android"], "assets/exports/android/drawable/settings.xml");
    }
}
