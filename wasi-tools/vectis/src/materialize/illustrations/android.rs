//! Android density PNG export for illustration vectors.

use std::path::Path;

use usvg::Tree;

use crate::materialize::paths::{
    ANDROID_DENSITIES, android_density_factor, android_raster_artifact_rel,
    resolve_under_assets_dir,
};
use crate::materialize::render::{render_tree_to_png, scaled_dimensions};

/// Write per-density PNG drawables for an illustration vector.
///
/// # Errors
///
/// Returns I/O or render errors from the underlying writes.
pub fn write_density_pngs(
    tree: &Tree, asset_id: &str, assets_dir: &Path, dry_run: bool,
) -> Result<Vec<String>, String> {
    let mut written = Vec::new();

    for density in ANDROID_DENSITIES {
        let rel = android_raster_artifact_rel(asset_id, density);
        written.push(rel.clone());

        if dry_run {
            continue;
        }

        let factor = android_density_factor(density).unwrap_or(1.0);
        let (width, height) = scaled_dimensions(tree, factor);
        let png = render_tree_to_png(tree, width, height)?;
        let out_path = resolve_under_assets_dir(assets_dir, &rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&out_path, png).map_err(|err| err.to_string())?;
    }

    Ok(written)
}
