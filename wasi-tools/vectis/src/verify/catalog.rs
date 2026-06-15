//! Shell asset-catalog completeness for `vectis verify --mode verify` (RFC-46 §7).
//!
//! Cross-checks composition-referenced `vector` / `raster` inventory against
//! on-disk shell resources (`Assets.xcassets` imagesets, Android `res/drawable*`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use specify_vectis_shell_detect::shell_present;

use crate::materialize::paths::{ANDROID_DENSITIES, kebab_to_snake};
use crate::validate::ValidateMode;
use crate::validate::engine::{
    collect_asset_references, discover_artifact, parse_yaml_file, resolve_default_path_with_root,
};

/// Collect diagnostic findings for composition-referenced non-symbol assets
/// that are absent from a present platform shell tree.
#[must_use]
pub fn catalog_findings(project_root: &Path, declared_platforms: &[String]) -> Vec<Value> {
    let assets_path = resolve_default_path_with_root(ValidateMode::Assets, project_root);
    let Some(assets_value) = parse_yaml_file(&assets_path) else {
        return Vec::new();
    };
    let Some(comp_path) = discover_artifact(&assets_path, ValidateMode::Composition) else {
        return Vec::new();
    };
    let Some(comp_value) = parse_yaml_file(&comp_path) else {
        return Vec::new();
    };

    let assets_map = assets_value.get("assets").and_then(Value::as_object);
    let refs = collect_asset_references(&comp_value);
    let mut seen = HashSet::new();
    let mut findings = Vec::new();

    for asset_ref in refs {
        if !seen.insert(asset_ref.id.clone()) {
            continue;
        }
        let Some(entry) = assets_map.and_then(|m| m.get(&asset_ref.id)) else {
            continue;
        };
        if !catalog_asset_applicable(entry) {
            continue;
        }
        for platform in declared_platforms {
            if !shell_present(project_root, platform) {
                continue;
            }
            if !is_supported_shell_platform(platform) {
                continue;
            }
            if shell_catalog_entry_present(project_root, platform, &asset_ref.id, entry) {
                continue;
            }
            findings.push(json!({
                "id": "shell-catalog-entry-missing",
                "severity": "error",
                "source": "deterministic",
                "message": format!(
                    "composition-referenced asset `{id}` (`kind: {kind}`) is missing from the {platform} shell catalog (expected {expectation})",
                    id = asset_ref.id,
                    kind = entry.get("kind").and_then(Value::as_str).unwrap_or("unknown"),
                    platform = platform,
                    expectation = catalog_expectation(platform, &asset_ref.id, entry),
                ),
            }));
        }
    }

    findings
}

fn catalog_asset_applicable(entry: &Value) -> bool {
    if entry.get("role").and_then(Value::as_str) == Some("app-icon") {
        return false;
    }
    matches!(
        entry.get("kind").and_then(Value::as_str),
        Some("vector" | "raster")
    )
}

fn is_supported_shell_platform(platform: &str) -> bool {
    matches!(platform, "ios" | "android")
}

fn catalog_expectation(platform: &str, asset_id: &str, entry: &Value) -> String {
    let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("asset");
    let role = entry.get("role").and_then(Value::as_str).unwrap_or("icon");
    match platform {
        "ios" => format!("`Assets.xcassets/{asset_id}.imageset/` with materialized content"),
        "android" if kind == "vector" && role != "illustration" => {
            let snake = kebab_to_snake(asset_id);
            format!("`res/drawable/{snake}.xml`")
        }
        "android" => {
            let snake = kebab_to_snake(asset_id);
            format!("`res/drawable-<density>/{snake}.png`")
        }
        _ => format!("shell-local catalog entry for `{asset_id}`"),
    }
}

fn shell_catalog_entry_present(project_root: &Path, platform: &str, asset_id: &str, entry: &Value) -> bool {
    match platform {
        "ios" => ios_shell_has_imageset(project_root, asset_id),
        "android" => android_shell_has_asset(project_root, asset_id, entry),
        _ => true,
    }
}

fn ios_shell_has_imageset(project_root: &Path, asset_id: &str) -> bool {
    ios_xcassets_roots(project_root).into_iter().any(|xcassets| {
        let imageset = xcassets.join(format!("{asset_id}.imageset"));
        imageset.is_dir() && directory_has_regular_file(&imageset)
    })
}

fn ios_xcassets_roots(project_dir: &Path) -> Vec<PathBuf> {
    let ios_root = project_dir.join("iOS");
    let Ok(entries) = std::fs::read_dir(&ios_root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path().join("Resources/Assets.xcassets"))
        .filter(|path| path.is_dir())
        .collect()
}

fn android_shell_has_asset(project_root: &Path, asset_id: &str, entry: &Value) -> bool {
    let res = project_root.join("Android/app/src/main/res");
    if !res.is_dir() {
        return false;
    }
    let snake = kebab_to_snake(asset_id);
    let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
    let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
    if kind == "vector" && role != "illustration" {
        return res.join(format!("drawable/{snake}.xml")).is_file();
    }
    android_shell_has_density_raster(&res, &snake)
}

fn android_shell_has_density_raster(res: &Path, snake: &str) -> bool {
    for density in ANDROID_DENSITIES {
        for ext in ["png", "jpg", "jpeg", "webp"] {
            if res.join(format!("drawable-{density}/{snake}.{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

fn directory_has_regular_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| entry.path().is_file())
}

#[cfg(test)]
mod tests;
