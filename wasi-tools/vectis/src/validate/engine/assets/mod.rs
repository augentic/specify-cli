//! `validate assets` — schema validation plus cross-artifact checks.

mod app_icon;
mod exports;
mod platforms;

use std::path::Path;

use serde_json::{Value, json};

use super::paths::{discover_artifact, find_project_root, resolve_default_path};
use super::shared::{assets_validator, escape_pointer_token, parse_yaml_file};
use crate::validate::ValidateMode;
use crate::validate::error::VectisError;

/// Validate `assets.yaml` against the embedded assets schema and
/// layer cross-artifact checks for resolved files and composition
/// references.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved file is
/// unreadable, and [`VectisError::Internal`] if the embedded schema
/// fails to compile.
pub(super) fn validate(path: Option<&Path>) -> Result<Value, VectisError> {
    let target = path
        .map_or_else(|| resolve_default_path(ValidateMode::Assets), std::path::Path::to_path_buf);

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("assets.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();

    let instance = match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => Some(instance),
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
            None
        }
    };

    if let Some(instance) = instance.as_ref() {
        let validator = assets_validator()?;
        for err in validator.iter_errors(instance) {
            errors.push(json!({
                "path": err.instance_path().to_string(),
                "message": err.to_string(),
            }));
        }

        let assets_dir = target.parent().unwrap_or_else(|| Path::new("."));
        let project_root = find_project_root(&target)
            .unwrap_or_else(|| assets_dir.parent().unwrap_or(assets_dir).to_path_buf());
        let shell_platforms = platforms::load_shell_platforms(&project_root);

        if let Some(assets) = instance.get("assets").and_then(Value::as_object) {
            for (id, entry) in assets {
                check_asset_files(id, entry, assets_dir, &mut errors, &mut warnings);
                app_icon::check_app_icon_entry(id, entry, &mut errors);
                if entry.get("kind").and_then(Value::as_str) == Some("raster")
                    && entry.get("role").and_then(Value::as_str) == Some("app-icon")
                    && let Some(source) = entry.get("source").and_then(Value::as_str)
                {
                    app_icon::check_raster_source_master(id, source, assets_dir, &mut errors);
                }
            }
            check_app_icon_pointer(instance, assets, &mut errors);
        }

        if let Some(comp_path) = discover_artifact(&target, ValidateMode::Composition)
            && let Some(comp_value) = parse_yaml_file(&comp_path)
        {
            let assets_map = instance.get("assets").and_then(Value::as_object);
            let refs = collect_asset_references(&comp_value);
            for asset_ref in &refs {
                let entry = assets_map.and_then(|m| m.get(&asset_ref.id));
                if entry.is_none() {
                    errors.push(json!({
                        "path": asset_ref.path,
                        "message": format!(
                            "composition.yaml at {} references unknown asset id `{}`",
                            comp_path.display(),
                            asset_ref.id,
                        ),
                    }));
                    continue;
                }
                let Some(entry) = entry else {
                    continue;
                };
                check_platform_coverage(
                    &asset_ref.id,
                    entry,
                    assets_dir,
                    &shell_platforms,
                    &mut errors,
                    &mut warnings,
                );
            }
        }
    }

    Ok(json!({
        "mode": ValidateMode::Assets.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    }))
}

fn check_asset_files(
    id: &str, entry: &Value, dir: &Path, errors: &mut Vec<Value>, warnings: &mut Vec<Value>,
) {
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    let app_icon = entry.get("role").and_then(Value::as_str) == Some("app-icon");
    let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
    match kind {
        "raster" => {
            if app_icon {
                if let Some(file) = entry.get("source").and_then(Value::as_str) {
                    check_file(&format!("/assets/{id}/source"), file, dir, errors);
                }
                for plat in ["ios", "android"] {
                    if let Some(path) =
                        entry.get("sources").and_then(|s| s.get(plat)).and_then(Value::as_str)
                    {
                        check_app_icon_platform_path(
                            &format!("/assets/{id}/sources/{plat}"),
                            path,
                            plat,
                            dir,
                            errors,
                        );
                    }
                }
            } else {
                for plat in ["ios", "android"] {
                    let densities =
                        entry.get("sources").and_then(|s| s.get(plat)).and_then(Value::as_object);
                    if let Some(map) = densities {
                        for (density, value) in map {
                            if let Some(file) = value.as_str() {
                                check_file(
                                    &format!("/assets/{id}/sources/{plat}/{density}"),
                                    file,
                                    dir,
                                    errors,
                                );
                            }
                        }
                    }
                    if plat == "ios" && role == "illustration" {
                        warn_illustration_ios_svg_paths(
                            id,
                            entry.get("sources").and_then(|s| s.get("ios")),
                            warnings,
                        );
                    }
                }
            }
        }
        "vector" => {
            if let Some(file) = entry.get("source").and_then(Value::as_str) {
                check_file(&format!("/assets/{id}/source"), file, dir, errors);
            }
            for plat in ["ios", "android"] {
                if let Some(path) =
                    entry.get("sources").and_then(|s| s.get(plat)).and_then(Value::as_str)
                {
                    if app_icon {
                        check_app_icon_platform_path(
                            &format!("/assets/{id}/sources/{plat}"),
                            path,
                            plat,
                            dir,
                            errors,
                        );
                    } else {
                        check_file(&format!("/assets/{id}/sources/{plat}"), path, dir, errors);
                        if plat == "ios"
                            && role == "illustration"
                            && source_extension(path).as_deref() == Some("svg")
                        {
                            warnings.push(json!({
                                "path": format!("/assets/{id}/sources/ios"),
                                "message": format!(
                                    "assets-svg-illustration-on-ios: illustration `{id}` uses `sources.ios` ending in `.svg` (prefer committed PNG exports on iOS)"
                                ),
                            }));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn check_app_icon_pointer(
    instance: &Value, assets: &serde_json::Map<String, Value>, errors: &mut Vec<Value>,
) {
    let Some(pointer) = instance.get("app-icon").and_then(Value::as_str) else {
        return;
    };
    let Some(entry) = assets.get(pointer) else {
        errors.push(json!({
            "path": "/app-icon",
            "message": format!(
                "assets-app-icon-invalid: top-level `app-icon` references unknown asset id `{pointer}`"
            ),
        }));
        return;
    };
    if entry.get("role").and_then(Value::as_str) != Some("app-icon") {
        errors.push(json!({
            "path": format!("/assets/{pointer}/role"),
            "message": format!(
                "assets-app-icon-invalid: asset `{pointer}` referenced by top-level `app-icon` must have `role: app-icon`"
            ),
        }));
    }
}

fn check_app_icon_platform_path(
    json_path: &str, path_rel: &str, platform: &str, dir: &Path, errors: &mut Vec<Value>,
) {
    let resolved = dir.join(path_rel);
    if resolved.is_file() {
        return;
    }
    if resolved.is_dir() {
        app_icon::check_export_layout(json_path, path_rel, platform, dir, errors);
        return;
    }
    if is_likely_export_root(path_rel) {
        errors.push(json!({
            "path": json_path,
            "message": format!(
                "assets-app-icon-export-invalid: export root not found: {}",
                resolved.display()
            ),
        }));
    } else {
        check_file(json_path, path_rel, dir, errors);
    }
}

fn is_likely_export_root(path_rel: &str) -> bool {
    let Some(ext) = source_extension(path_rel) else {
        return true;
    };
    !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "svg" | "pdf" | "xml")
}

fn check_file(json_path: &str, file_rel: &str, dir: &Path, errors: &mut Vec<Value>) {
    let resolved = dir.join(file_rel);
    if !resolved.is_file() {
        errors.push(json!({
            "path": json_path,
            "message": format!("file not found: {}", resolved.display()),
        }));
    }
}

fn check_platform_coverage(
    id: &str, entry: &Value, assets_dir: &Path, platforms: &[String], errors: &mut Vec<Value>,
    warnings: &mut Vec<Value>,
) {
    if entry.get("role").and_then(Value::as_str) == Some("app-icon") {
        return;
    }
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "raster" => {
            for plat in platforms {
                let plat_node = entry.get("sources").and_then(|s| s.get(plat.as_str()));
                if plat_node.is_some() {
                    if let Some(map) = plat_node.and_then(Value::as_object) {
                        for &density in raster_densities(plat) {
                            if !map.contains_key(density) {
                                warnings.push(json!({
                                    "path": format!("/assets/{id}/sources/{plat}"),
                                    "message": format!(
                                        "raster asset `{id}` is missing optional `{density}` density for {plat}"
                                    ),
                                }));
                            }
                        }
                    }
                    continue;
                }
                if exports::conventional_export_exists(assets_dir, id, kind, plat) {
                    continue;
                }
                errors.push(json!({
                    "path": format!("/assets/{id}/sources/{plat}"),
                    "message": format!(
                        "assets-materialization-missing: raster asset `{id}` is referenced by composition.yaml but has no `sources.{plat}` pin and no committed export under `assets/exports/{plat}/`"
                    ),
                }));
            }
        }
        "vector" => {
            for plat in platforms {
                let has_pin = entry.get("sources").and_then(|s| s.get(plat.as_str())).is_some();
                if has_pin {
                    continue;
                }
                if exports::vector_source_materializable(assets_dir, entry)
                    || exports::conventional_export_exists(assets_dir, id, kind, plat)
                {
                    continue;
                }
                errors.push(json!({
                    "path": format!("/assets/{id}/sources/{plat}"),
                    "message": format!(
                        "assets-materialization-missing: vector asset `{id}` is referenced by composition.yaml but has no `sources.{plat}` pin, no materializable `source:`, and no committed export under `assets/exports/{plat}/`"
                    ),
                }));
            }
        }
        _ => {}
    }
}

fn warn_illustration_ios_svg_paths(id: &str, ios_node: Option<&Value>, warnings: &mut Vec<Value>) {
    let Some(ios_node) = ios_node else {
        return;
    };
    let paths: Vec<&str> = if let Some(path) = ios_node.as_str() {
        vec![path]
    } else if let Some(map) = ios_node.as_object() {
        map.values().filter_map(Value::as_str).collect()
    } else {
        return;
    };
    if paths.iter().any(|path| source_extension(path).as_deref() == Some("svg")) {
        warnings.push(json!({
            "path": format!("/assets/{id}/sources/ios"),
            "message": format!(
                "assets-svg-illustration-on-ios: illustration `{id}` uses `sources.ios` ending in `.svg` (prefer committed PNG exports on iOS)"
            ),
        }));
    }
}

fn source_extension(path: &str) -> Option<String> {
    Path::new(path).extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase)
}

const fn raster_densities(plat: &str) -> &'static [&'static str] {
    match plat.as_bytes() {
        b"ios" => &["1x", "2x", "3x"],
        b"android" => &["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"],
        _ => &[],
    }
}

pub(super) struct AssetRef {
    pub(super) id: String,
    pub(super) path: String,
}

pub(super) fn collect_asset_references(value: &Value) -> Vec<AssetRef> {
    let mut refs = Vec::new();
    walk_node(value, "", &mut refs);
    refs
}

fn walk_node(node: &Value, json_path: &str, refs: &mut Vec<AssetRef>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                match key.as_str() {
                    "image" | "icon" => {
                        if let Some(name) = val.get("name").and_then(Value::as_str) {
                            refs.push(AssetRef {
                                id: name.to_string(),
                                path: format!("{child_path}/name"),
                            });
                        }
                    }
                    "icon-button" | "fab" => {
                        if let Some(icon) = val.get("icon").and_then(Value::as_str) {
                            refs.push(AssetRef {
                                id: icon.to_string(),
                                path: format!("{child_path}/icon"),
                            });
                        }
                    }
                    _ => {}
                }
                walk_node(val, &child_path, refs);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_node(v, &format!("{json_path}/{i}"), refs);
            }
        }
        _ => {}
    }
}
