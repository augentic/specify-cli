//! `validate assets` — schema validation plus three cross-artifact
//! checks (file existence, composition discovery, and per-platform
//! source coverage for composition-referenced assets).

use std::path::Path;

use serde_json::{Value, json};

use super::paths::{discover_artifact, resolve_default_path};
use super::shared::{assets_validator, escape_pointer_token, parse_yaml_file};
use crate::error::VectisError;
use crate::{CommandOutcome, ValidateMode};

/// Validate `assets.yaml` against the embedded assets schema and
/// layer the cross-artifact checks for resolved files and composition
/// references.
///
/// On top of schema validation the function performs three
/// cross-artifact checks:
///
/// 1. **File existence**: every raster density entry, every vector
///    `source`, and every vector `sources.<platform>` is resolved
///    relative to the directory containing `assets.yaml`. Missing
///    files become errors with a JSON-Pointer-shaped `path`.
/// 2. **Composition discovery**: the unified resolver picks up the
///    nearest sibling composition. If no composition is found, the
///    cross-artifact checks below are skipped silently.
/// 3. **Cross-artifact reference checks**: every `image`, `icon`,
///    `icon-button`, and `fab` asset reference in the discovered
///    composition is resolved against the asset id set. Unknown ids
///    become errors. For raster + vector assets that ARE referenced,
///    both `sources.ios` and `sources.android` must be present
///    (missing platform = error); raster assets surface a warning
///    per missing optional density slot when the platform itself is
///    populated.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved file is
/// unreadable, and [`VectisError::Internal`] if the embedded schema
/// fails to compile.
pub(super) fn validate(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
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

        if let Some(assets) = instance.get("assets").and_then(Value::as_object) {
            for (id, entry) in assets {
                check_asset_files(id, entry, assets_dir, &mut errors);
            }
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
                check_platform_coverage(&asset_ref.id, entry, &mut errors, &mut warnings);
            }
        }
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::Assets.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    })))
}

/// Walk a single asset entry's filePaths and append a "file not found"
/// error for each one that does not resolve to a regular file under
/// `dir`. Symbol assets carry no filePaths so they are a no-op here.
/// Schema-invalid entries (missing or non-string `kind`, non-object
/// `sources`, etc.) are skipped silently because the schema validator
/// already reported them; this function is a best-effort second pass
/// over what the schema accepts.
fn check_asset_files(id: &str, entry: &Value, dir: &Path, errors: &mut Vec<Value>) {
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "raster" => {
            for plat in PLATFORMS {
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
            }
        }
        "vector" => {
            if let Some(file) = entry.get("source").and_then(Value::as_str) {
                check_file(&format!("/assets/{id}/source"), file, dir, errors);
            }
            for plat in PLATFORMS {
                if let Some(file) =
                    entry.get("sources").and_then(|s| s.get(plat)).and_then(Value::as_str)
                {
                    check_file(&format!("/assets/{id}/sources/{plat}"), file, dir, errors);
                }
            }
        }
        _ => {}
    }
}

/// Resolve `file_rel` (a path relative to the directory containing
/// `assets.yaml`) and append an error to `errors` when the path does
/// not exist on disk or is not a regular file.
fn check_file(json_path: &str, file_rel: &str, dir: &Path, errors: &mut Vec<Value>) {
    let resolved = dir.join(file_rel);
    if !resolved.is_file() {
        errors.push(json!({
            "path": json_path,
            "message": format!("file not found: {}", resolved.display()),
        }));
    }
}

/// Per-platform source coverage for a composition-referenced asset.
/// V1 conservatively checks both `ios` and `android`; the formal
/// "targeted shell platforms" wiring (driven by the proposal's
/// `Platforms` field) is deferred to a later phase.
///
/// - **Raster**: `sources.<plat>` must be present (else "no usable
///   source" → error). When present, every density slot the schema
///   recognises but the entry omits is a warning.
/// - **Vector**: `sources.<plat>` (a single filePath) must be
///   present; the canonical `source` does not satisfy a per-platform
///   reference.
/// - **Symbol**: the schema already requires `symbols.<plat>` to be
///   non-empty when present; per-platform symbol coverage is
///   intentionally not enforced here (it lives in composition mode,
///   which has the proposal context to know which platforms are
///   targeted).
fn check_platform_coverage(
    id: &str, entry: &Value, errors: &mut Vec<Value>, warnings: &mut Vec<Value>,
) {
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "raster" => {
            for plat in PLATFORMS {
                let plat_node = entry.get("sources").and_then(|s| s.get(plat));
                let Some(plat_node) = plat_node else {
                    errors.push(json!({
                        "path": format!("/assets/{id}/sources/{plat}"),
                        "message": format!(
                            "raster asset `{id}` is referenced by composition.yaml but has no `sources.{plat}` source for the targeted shell platform"
                        ),
                    }));
                    continue;
                };
                if let Some(map) = plat_node.as_object() {
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
            }
        }
        "vector" => {
            for plat in PLATFORMS {
                let plat_node = entry.get("sources").and_then(|s| s.get(plat));
                if plat_node.is_none() {
                    errors.push(json!({
                        "path": format!("/assets/{id}/sources/{plat}"),
                        "message": format!(
                            "vector asset `{id}` is referenced by composition.yaml but has no `sources.{plat}` export for the targeted shell platform"
                        ),
                    }));
                }
            }
        }
        _ => {}
    }
}

/// Platform set v1 conservatively considers "targeted". When a later
/// phase wires the build brief, the actual platform set comes from the
/// proposal's `Platforms` field; this constant becomes the fallback.
const PLATFORMS: [&str; 2] = ["ios", "android"];

/// Raster density slot order per platform. Matches the property shape
/// `assets.schema.json` accepts on `rasterEntry.sources.<plat>`. The
/// order here is the one warnings render in.
const fn raster_densities(plat: &str) -> &'static [&'static str] {
    match plat.as_bytes() {
        b"ios" => &["1x", "2x", "3x"],
        b"android" => &["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"],
        _ => &[],
    }
}

/// Recorded asset reference from a composition document. The `path`
/// is a JSON-Pointer-shaped indicator that points at the source of the
/// reference inside `composition.yaml`.
pub(super) struct AssetRef {
    /// Asset id the composition references (the kebab-case key under
    /// `assets:` it expects to find).
    pub(super) id: String,
    /// JSON-Pointer-shaped location of the reference inside the
    /// composition document.
    pub(super) path: String,
}

/// Walk a composition document and collect every static asset
/// reference (`image`, `icon`, `icon-button`, `fab`). Dynamic
/// references (`bind: assets.<id>`) are out of scope; composition
/// mode's bind resolver handles them separately.
pub(super) fn collect_asset_references(value: &Value) -> Vec<AssetRef> {
    let mut refs = Vec::new();
    walk_node(value, "", &mut refs);
    refs
}

/// Recursive walker driving [`collect_asset_references`]. We match
/// only the four item-type / region keys that point at a static asset
/// id in v1 to keep the walker tight; the recursion still descends
/// into every value so nested groups, overlay content, state-replaced
/// bodies, and `platforms.*` overrides are all covered.
fn walk_node(node: &Value, json_path: &str, refs: &mut Vec<AssetRef>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                match key.as_str() {
                    // `image:` and `icon:` item types: the asset id
                    // lives under `name:`. The string-shorthand form
                    // (`image: foo`) is intentionally ignored because
                    // the v1 schema requires the object form for both
                    // items, and accepting shorthand would double-count
                    // the `icon: <string>` property inside
                    // `icon-button` / `fab`.
                    "image" | "icon" => {
                        if let Some(name) = val.get("name").and_then(Value::as_str) {
                            refs.push(AssetRef {
                                id: name.to_string(),
                                path: format!("{child_path}/name"),
                            });
                        }
                    }
                    // `icon-button:` and `fab:` carry the asset id
                    // directly under `icon:`.
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
