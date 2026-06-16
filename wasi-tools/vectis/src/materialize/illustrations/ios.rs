//! iOS PNG imageset export for illustration vectors.

use std::path::Path;

use serde_json::json;
use usvg::Tree;

use crate::materialize::paths::{
    IOS_ILLUSTRATION_SCALES, ios_imageset_dir, ios_raster_artifact_rel, ios_raster_filename,
    ios_scale_factor, resolve_under_assets_dir,
};
use crate::materialize::render::{render_tree_to_png, scaled_dimensions};

/// Write an iOS imageset with `@2x` / `@3x` PNGs and `Contents.json`.
///
/// # Errors
///
/// Returns I/O or render errors from the underlying writes.
pub fn write_imageset(
    tree: &Tree, asset_id: &str, assets_dir: &Path, imageset_dir: &Path, dry_run: bool,
) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    let mut images = Vec::new();

    for scale in IOS_ILLUSTRATION_SCALES {
        let rel = ios_raster_artifact_rel(asset_id, scale);
        written.push(rel.clone());
        images.push(json!({
            "filename": ios_raster_filename(asset_id, scale),
            "idiom": "universal",
            "scale": scale,
        }));

        if dry_run {
            continue;
        }

        let factor = ios_scale_factor(scale).ok_or_else(|| format!("unsupported iOS scale `{scale}`"))?;
        let (width, height) = scaled_dimensions(tree, factor);
        let png = render_tree_to_png(tree, width, height)?;
        let out_path = resolve_under_assets_dir(assets_dir, &rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&out_path, png).map_err(|err| err.to_string())?;
    }

    let contents_rel = format!("{}/Contents.json", ios_imageset_dir(asset_id));
    written.push(contents_rel.clone());

    if !dry_run {
        std::fs::create_dir_all(imageset_dir).map_err(|err| err.to_string())?;
        let contents = json!({
            "images": images,
            "info": {
                "author": "vectis",
                "version": 1
            }
        });
        let contents_path = resolve_under_assets_dir(assets_dir, &contents_rel);
        std::fs::write(
            contents_path,
            serde_json::to_vec_pretty(&contents).expect("contents json"),
        )
        .map_err(|err| err.to_string())?;
    }

    Ok(written)
}
