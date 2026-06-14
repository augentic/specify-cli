//! Resolve targeted shell platforms from `.specify/project.yaml`.

use std::path::Path;

use serde_json::Value;

/// Shell platforms that carry per-platform asset exports in `assets.yaml`.
const ASSET_SHELL_PLATFORMS: &[&str] = &["ios", "android"];

/// Load `ios` / `android` entries declared in `project.yaml.platforms`.
///
/// When the config is absent or invalid, returns the historical
/// fallback `["ios", "android"]` so standalone `validate assets`
/// invocations against a lone `assets.yaml` keep working.
pub(super) fn load_shell_platforms(project_root: &Path) -> Vec<String> {
    let config_path = project_root.join(".specify").join("project.yaml");
    let Ok(source) = std::fs::read_to_string(&config_path) else {
        return fallback_platforms();
    };
    let doc: Value = match serde_saphyr::from_str(&source) {
        Ok(doc) => doc,
        Err(_) => return fallback_platforms(),
    };
    let Some(platforms) = doc.get("platforms").and_then(Value::as_array) else {
        return fallback_platforms();
    };

    let mut shell: Vec<String> = Vec::new();
    for entry in platforms {
        let Some(name) = entry.as_str() else {
            continue;
        };
        if ASSET_SHELL_PLATFORMS.contains(&name) && !shell.iter().any(|p| p == name) {
            shell.push(name.to_string());
        }
    }
    if shell.is_empty() { fallback_platforms() } else { shell }
}

fn fallback_platforms() -> Vec<String> {
    vec!["ios".to_string(), "android".to_string()]
}
