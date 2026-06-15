//! Conventional committed-export presence for materialization checks.

use std::path::Path;

use serde_json::Value;

use crate::materialize::paths::{export_layout, kebab_to_snake, Platform};

/// Whether a composition-referenced asset has a committed export on
/// disk for `platform` without a `sources.<platform>` pin.
///
/// Auto-materializable vector roles are checked against the same
/// conventional paths `materialize assets` writes; raster entries
/// without an auto-convert layout fall back to imageset / density
/// heuristics.
pub(super) fn conventional_export_exists(
    assets_dir: &Path, id: &str, kind: &str, platform: &str, entry: &Value,
) -> bool {
    let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
    if let Some(plat) = Platform::parse(platform)
        && let Some(layout) = export_layout(role, kind, plat, id)
    {
        return layout
            .artifacts
            .iter()
            .any(|rel| assets_dir.join(rel).is_file());
    }
    conventional_raster_export_exists(assets_dir, id, kind, platform)
}

fn conventional_raster_export_exists(
    assets_dir: &Path, id: &str, kind: &str, platform: &str,
) -> bool {
    let exports_root = assets_dir.join("assets/exports").join(platform);
    if !exports_root.is_dir() {
        return false;
    }
    match (platform, kind) {
        ("ios", "raster") => {
            let imageset = exports_root.join(format!("{id}.imageset"));
            imageset.is_dir() && imageset_has_materialized_content(&imageset)
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

/// Whether an iOS imageset directory carries materialized content.
///
/// `Contents.json` alone does not satisfy export presence (RFC §6.3).
pub(crate) fn imageset_has_materialized_content(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path.is_file() && entry.file_name() != "Contents.json"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn icon_vector_android_export_matches_materialize_layout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let design = tmp.path();
        let xml = design.join("assets/exports/android/drawable/chevron_right.xml");
        std::fs::create_dir_all(xml.parent().expect("parent")).expect("mkdir");
        std::fs::write(&xml, "<vector/>").expect("write");
        let entry = json!({ "role": "icon", "kind": "vector" });
        assert!(conventional_export_exists(design, "chevron-right", "vector", "android", &entry));
    }

    #[test]
    fn contents_json_only_imageset_is_not_materialized() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let design = tmp.path();
        let imageset = design.join("assets/exports/ios/hero.imageset");
        std::fs::create_dir_all(&imageset).expect("mkdir");
        std::fs::write(imageset.join("Contents.json"), "{\"images\":[]}").expect("write json");
        assert!(!imageset_has_materialized_content(&imageset));
    }

    #[test]
    fn raster_ios_imageset_requires_materialized_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let design = tmp.path();
        let imageset = design.join("assets/exports/ios/hero.imageset");
        std::fs::create_dir_all(&imageset).expect("mkdir");
        std::fs::write(imageset.join("Contents.json"), "{\"images\":[]}").expect("write json");
        let entry = json!({ "role": "illustration", "kind": "raster" });
        assert!(!conventional_export_exists(design, "hero", "raster", "ios", &entry));

        std::fs::write(imageset.join("hero@3x.png"), b"PNG").expect("write png");
        assert!(conventional_export_exists(design, "hero", "raster", "ios", &entry));
    }

    #[test]
    fn raster_android_density_png_satisfies_export() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let design = tmp.path();
        let png = design.join("assets/exports/android/drawable-mdpi/hero.png");
        std::fs::create_dir_all(png.parent().expect("parent")).expect("mkdir");
        std::fs::write(&png, b"PNG").expect("write");
        let entry = json!({ "role": "illustration", "kind": "raster" });
        assert!(conventional_export_exists(design, "hero", "raster", "android", &entry));
    }
}

