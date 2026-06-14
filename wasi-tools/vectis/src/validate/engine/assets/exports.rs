//! Conventional committed-export presence for materialization checks.

use std::path::Path;

/// Whether a composition-referenced asset has a committed export on
/// disk for `platform` without a `sources.<platform>` pin.
pub(super) fn conventional_export_exists(
    assets_dir: &Path, id: &str, kind: &str, platform: &str,
) -> bool {
    let exports_root = assets_dir.join("assets/exports").join(platform);
    if !exports_root.is_dir() {
        return false;
    }
    match (platform, kind) {
        ("ios", "vector" | "raster") => {
            let imageset = exports_root.join(format!("{id}.imageset"));
            imageset.is_dir() && directory_has_regular_file(&imageset)
        }
        ("android", "vector") => {
            let snake = kebab_to_snake(id);
            exports_root.join("drawable").join(format!("{snake}.xml")).is_file()
        }
        ("android", "raster") => {
            let snake = kebab_to_snake(id);
            for density in ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"] {
                if exports_root
                    .join(format!("drawable-{density}"))
                    .join(format!("{snake}.png"))
                    .is_file()
                {
                    return true;
                }
                if exports_root
                    .join(format!("mipmap-{density}"))
                    .join(format!("{snake}.png"))
                    .is_file()
                {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Path A for vector inventory: canonical `source:` exists on disk.
pub(super) fn vector_source_materializable(assets_dir: &Path, entry: &serde_json::Value) -> bool {
    entry
        .get("source")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|source| assets_dir.join(source).is_file())
}

fn directory_has_regular_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|e| e.path().is_file())
}

fn kebab_to_snake(id: &str) -> String {
    id.replace('-', "_")
}
