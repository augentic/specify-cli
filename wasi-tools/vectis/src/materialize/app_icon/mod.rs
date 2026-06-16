//! App-icon materialization — shared launcher canvas and per-platform exports.

mod android;
mod canvas;
mod ios;

use std::path::Path;

use serde_json::{Value, json};

use crate::materialize::icons::{active_platform_pin, asset_error, materialized_entry};
use crate::materialize::paths::{Platform, export_layout, resolve_under_assets_dir};

pub use canvas::{LAUNCHER_CANVAS_SIZE, decode_to_launcher_canvas};

/// Materialize `role: app-icon` entries with a canonical `source:` master.
pub fn materialize_app_icons(
    assets_dir: &Path, assets: &serde_json::Map<String, Value>, platforms: &[String],
    dry_run: bool, materialized: &mut Vec<Value>, skipped_pins: &mut Vec<Value>,
    errors: &mut Vec<Value>,
) {
    for (asset_id, entry) in assets {
        if entry.get("role").and_then(Value::as_str) != Some("app-icon") {
            continue;
        }
        let Some(source_rel) = entry.get("source").and_then(Value::as_str) else {
            continue;
        };
        let source_path = assets_dir.join(source_rel);

        let canvas = match decode_to_launcher_canvas(&source_path, source_rel, asset_id) {
            Ok(canvas) => canvas,
            Err(message) => {
                errors.push(asset_error(asset_id, &message));
                continue;
            }
        };

        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("vector");

        for platform_name in platforms {
            let Some(platform) = Platform::parse(platform_name) else {
                continue;
            };
            if let Some(pin) = active_platform_pin(entry, platform_name, assets_dir) {
                skipped_pins.push(json!({
                    "asset_id": asset_id,
                    "platform": platform_name,
                    "pin": pin,
                }));
                continue;
            }

            let Some(layout) = export_layout("app-icon", kind, platform, asset_id) else {
                continue;
            };

            let result = match platform {
                Platform::Ios => materialize_ios(asset_id, assets_dir, &layout, &canvas, dry_run),
                Platform::Android => materialize_android(
                    asset_id,
                    entry,
                    assets_dir,
                    &layout,
                    &canvas,
                    dry_run,
                ),
            };
            match result {
                Ok(written) => materialized.extend(written),
                Err(message) => errors.push(asset_error(asset_id, &message)),
            }
        }
    }
}

fn materialize_ios(
    asset_id: &str, assets_dir: &Path, layout: &crate::materialize::paths::ExportLayout,
    canvas: &image::RgbaImage, dry_run: bool,
) -> Result<Vec<Value>, String> {
    if dry_run {
        return Ok(layout
            .artifacts
            .iter()
            .map(|path| materialized_entry(asset_id, Platform::Ios, path))
            .collect());
    }

    let appiconset_dir = resolve_under_assets_dir(assets_dir, &layout.pin);
    ios::write_appiconset(canvas, &appiconset_dir).map_err(|err| {
        format!("asset `{asset_id}`: iOS app-icon export failed: {err}")
    })?;

    Ok(layout
        .artifacts
        .iter()
        .map(|path| materialized_entry(asset_id, Platform::Ios, path))
        .collect())
}

fn materialize_android(
    asset_id: &str, entry: &Value, assets_dir: &Path,
    layout: &crate::materialize::paths::ExportLayout, canvas: &image::RgbaImage, dry_run: bool,
) -> Result<Vec<Value>, String> {
    if dry_run {
        return Ok(layout
            .artifacts
            .iter()
            .map(|path| materialized_entry(asset_id, Platform::Android, path))
            .collect());
    }

    let export_root = resolve_under_assets_dir(assets_dir, &layout.pin);
    let background = android::resolve_launcher_background(entry, assets_dir);
    android::write_android_export(canvas, &background, &export_root).map_err(|err| {
        format!("asset `{asset_id}`: Android app-icon export failed: {err}")
    })?;

    Ok(layout
        .artifacts
        .iter()
        .map(|path| materialized_entry(asset_id, Platform::Android, path))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use image::{ImageFormat, Rgba, RgbaImage};
    use tempfile::tempdir;

    const SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <rect width="1024" height="1024" fill="#112233"/>
</svg>"##;

    #[test]
    fn materialize_app_icon_ios_from_svg() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(design.join("assets")).expect("assets dir");
        fs::write(design.join("assets/app-icon.svg"), SQUARE_SVG).expect("svg");

        let yaml = r#"version: 1
app-icon: app-icon
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.svg
"#;
        fs::write(design.join("assets.yaml"), yaml).expect("yaml");

        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets");

        let mut materialized = Vec::new();
        let mut skipped = Vec::new();
        let mut errors = Vec::new();
        materialize_app_icons(
            &design,
            assets,
            &["ios".into()],
            false,
            &mut materialized,
            &mut skipped,
            &mut errors,
        );
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let appiconset = design.join("assets/exports/ios/app-icon/AppIcon.appiconset");
        let png = appiconset.join(ios::APPICON_PNG_NAME);
        let contents = appiconset.join("Contents.json");
        assert!(png.is_file() && contents.is_file());
        assert!(serde_json::from_slice::<Value>(&fs::read(&contents).expect("read")).is_ok());

        let decoded = image::ImageReader::open(&png).expect("open").decode().expect("decode");
        assert_eq!(decoded.width(), 1024);
        assert_eq!(decoded.height(), 1024);
    }

    #[test]
    fn materialize_app_icon_android_from_svg() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(design.join("assets")).expect("assets dir");
        fs::write(design.join("assets/app-icon.svg"), SQUARE_SVG).expect("svg");
        fs::write(
            design.join("tokens.yaml"),
            "version: 1\ncolors:\n  surface:\n    light: \"#EEF0F2\"\n    dark: \"#111111\"\n",
        )
        .expect("tokens");

        let yaml = r#"version: 1
app-icon: app-icon
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "App icon"
    tint: surface
    source: assets/app-icon.svg
"#;
        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets");

        let mut materialized = Vec::new();
        let mut skipped = Vec::new();
        let mut errors = Vec::new();
        materialize_app_icons(
            &design,
            assets,
            &["android".into()],
            false,
            &mut materialized,
            &mut skipped,
            &mut errors,
        );
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let root = design.join("assets/exports/android/app-icon");
        assert!(root.join("mipmap-anydpi-v26/ic_launcher.xml").is_file());
        assert!(root.join("drawable-xxhdpi/ic_launcher_foreground.png").is_file());
        assert!(root.join("mipmap-mdpi/ic_launcher.png").is_file());

        let bg = fs::read_to_string(root.join("values/ic_launcher_background.xml")).expect("bg");
        assert!(bg.contains("#EEF0F2"));

        let launcher =
            fs::read_to_string(root.join("mipmap-anydpi-v26/ic_launcher.xml")).expect("xml");
        assert!(launcher.contains("adaptive-icon"));
        assert!(!materialized.is_empty());
    }

    #[test]
    fn materialize_app_icon_skips_pinned_android_export() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        let export_root = design.join("assets/exports/android/app-icon");
        fs::create_dir_all(export_root.join("mipmap-anydpi-v26")).expect("mkdir");
        fs::write(export_root.join("mipmap-anydpi-v26/ic_launcher.xml"), "<adaptive-icon/>")
            .expect("launcher");
        fs::write(design.join("assets/app-icon.svg"), SQUARE_SVG).expect("svg");

        let yaml = r#"version: 1
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.svg
    sources:
      android: assets/exports/android/app-icon
"#;
        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets");

        let mut materialized = Vec::new();
        let mut skipped = Vec::new();
        materialize_app_icons(
            &design,
            assets,
            &["android".into()],
            false,
            &mut materialized,
            &mut skipped,
            &mut Vec::new(),
        );
        assert!(materialized.is_empty());
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn materialize_app_icon_skips_pinned_ios_export() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        let appiconset = design.join("assets/exports/ios/app-icon/AppIcon.appiconset");
        fs::create_dir_all(&appiconset).expect("mkdir");
        fs::write(
            appiconset.join("Contents.json"),
            r#"{"images":[{"filename":"AppIcon.png","idiom":"universal","platform":"ios","size":"1024x1024"}],"info":{"version":1,"author":"xcode"}}"#,
        )
        .expect("contents");
        fs::write(design.join("assets/app-icon.svg"), SQUARE_SVG).expect("svg");

        let yaml = r#"version: 1
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.svg
    sources:
      ios: assets/exports/ios/app-icon/AppIcon.appiconset
"#;
        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets");

        let mut materialized = Vec::new();
        let mut skipped = Vec::new();
        materialize_app_icons(
            &design,
            assets,
            &["ios".into()],
            false,
            &mut materialized,
            &mut skipped,
            &mut Vec::new(),
        );
        assert!(materialized.is_empty());
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn materialize_app_icon_rejects_small_raster_master() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(design.join("assets")).expect("assets dir");
        let png_path = design.join("assets/app-icon.png");
        RgbaImage::from_pixel(512, 512, Rgba([1, 2, 3, 255]))
            .save_with_format(&png_path, ImageFormat::Png)
            .expect("png");

        let yaml = r#"version: 1
assets:
  app-icon:
    kind: raster
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.png
"#;
        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets");

        let mut errors = Vec::new();
        materialize_app_icons(
            &design,
            assets,
            &["ios".into()],
            false,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0]["message"]
                .as_str()
                .unwrap_or("")
                .contains("assets-app-icon-source-invalid")
        );
    }
}
