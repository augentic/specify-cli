//! Illustration vector materialization — SVG to per-density PNG exports.

mod android;
mod ios;

use std::path::Path;

use serde_json::{Value, json};

use crate::materialize::icons::{active_platform_pin, asset_error, materialized_entry};
use crate::materialize::paths::{Platform, export_layout, ios_imageset_dir, resolve_under_assets_dir};
use crate::materialize::svg::parse_icon_svg;

/// Materialize every in-scope `role: illustration` vector entry from `source:`.
pub fn materialize_illustration_vectors(
    assets_dir: &Path, assets: &serde_json::Map<String, Value>, platforms: &[String],
    dry_run: bool, materialized: &mut Vec<Value>, skipped_pins: &mut Vec<Value>,
    errors: &mut Vec<Value>,
) {
    for (asset_id, entry) in assets {
        if !is_illustration_vector_entry(entry) {
            continue;
        }
        let Some(source_rel) = entry.get("source").and_then(Value::as_str) else {
            continue;
        };
        let source_path = assets_dir.join(source_rel);
        let svg_bytes = match std::fs::read(&source_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                errors.push(asset_error(asset_id, &format!("source not readable at {source_rel}: {err}")));
                continue;
            }
        };

        let parsed = match parse_icon_svg(&svg_bytes, asset_id) {
            Ok(parsed) => parsed,
            Err(message) => {
                errors.push(asset_error(asset_id, &message));
                continue;
            }
        };

        for platform_name in platforms {
            if let Some(pin) = active_platform_pin(entry, platform_name, assets_dir) {
                skipped_pins.push(json!({
                    "asset_id": asset_id,
                    "platform": platform_name,
                    "pin": pin,
                }));
                continue;
            }

            let Some(platform) = Platform::parse(platform_name) else {
                continue;
            };
            let Some(layout) = export_layout("illustration", "vector", platform, asset_id) else {
                continue;
            };

            match materialize_for_platform(
                &parsed.tree,
                asset_id,
                platform,
                assets_dir,
                &layout,
                dry_run,
            ) {
                Ok(written) => {
                    for path in written {
                        materialized.push(materialized_entry(asset_id, platform, &path));
                    }
                }
                Err(message) => errors.push(asset_error(asset_id, &message)),
            }
        }
    }
}

fn materialize_for_platform(
    tree: &usvg::Tree, asset_id: &str, platform: Platform, assets_dir: &Path,
    layout: &crate::materialize::paths::ExportLayout, dry_run: bool,
) -> Result<Vec<String>, String> {
    match platform {
        Platform::Ios => {
            let imageset_dir = resolve_under_assets_dir(assets_dir, &ios_imageset_dir(asset_id));
            if dry_run {
                return Ok(layout.artifacts.clone());
            }
            ios::write_imageset(tree, asset_id, assets_dir, &imageset_dir, dry_run)
        }
        Platform::Android => {
            if dry_run {
                return Ok(layout.artifacts.clone());
            }
            android::write_density_pngs(tree, asset_id, assets_dir, dry_run)
        }
    }
}

fn is_illustration_vector_entry(entry: &Value) -> bool {
    entry.get("role").and_then(Value::as_str) == Some("illustration")
        && entry.get("kind").and_then(Value::as_str) == Some("vector")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use image::ImageReader;
    use tempfile::tempdir;

    const TRIANGLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="#010203" d="M12 2L2 22h20z"/>
</svg>"##;

    #[test]
    fn materialize_illustration_writes_scaled_pngs() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(design.join("assets")).expect("assets dir");
        fs::write(design.join("assets/onboarding-hero.svg"), TRIANGLE).expect("svg");

        let yaml = r#"version: 1
assets:
  onboarding-hero:
    kind: vector
    role: illustration
    alt: "Hero"
    source: assets/onboarding-hero.svg
"#;
        fs::write(design.join("assets.yaml"), yaml).expect("yaml");

        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets map");

        let mut materialized = Vec::new();
        let mut skipped = Vec::new();
        let mut errors = Vec::new();
        materialize_illustration_vectors(
            &design,
            assets,
            &["ios".into(), "android".into()],
            false,
            &mut materialized,
            &mut skipped,
            &mut errors,
        );
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let ios_2x = design.join("assets/exports/ios/onboarding-hero.imageset/onboarding-hero@2x.png");
        let ios_3x = design.join("assets/exports/ios/onboarding-hero.imageset/onboarding-hero@3x.png");
        let android_mdpi =
            design.join("assets/exports/android/drawable-mdpi/onboarding_hero.png");
        let android_xxxhdpi =
            design.join("assets/exports/android/drawable-xxxhdpi/onboarding_hero.png");

        assert!(ios_2x.is_file() && ios_3x.is_file());
        assert!(android_mdpi.is_file() && android_xxxhdpi.is_file());

        let img_2x = ImageReader::open(&ios_2x).expect("open 2x").decode().expect("decode 2x");
        let img_3x = ImageReader::open(&ios_3x).expect("open 3x").decode().expect("decode 3x");
        assert_eq!(img_2x.width(), 48);
        assert_eq!(img_2x.height(), 48);
        assert_eq!(img_3x.width(), 72);
        assert_eq!(img_3x.height(), 72);

        let img_mdpi =
            ImageReader::open(&android_mdpi).expect("open mdpi").decode().expect("decode mdpi");
        let img_xxxhdpi = ImageReader::open(&android_xxxhdpi)
            .expect("open xxxhdpi")
            .decode()
            .expect("decode xxxhdpi");
        assert_eq!(img_mdpi.width(), 24);
        assert_eq!(img_mdpi.height(), 24);
        assert_eq!(img_xxxhdpi.width(), 96);
        assert_eq!(img_xxxhdpi.height(), 96);
    }
}
