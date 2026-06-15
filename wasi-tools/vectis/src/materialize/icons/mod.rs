//! Icon vector materialization — SVG to iOS PDF imageset and Android VD XML.

mod android;
mod ios;
mod pdf;

use std::path::Path;

use serde_json::{Value, json};
use usvg::Tree;

use crate::materialize::paths::{Platform, export_layout, ios_imageset_dir, resolve_under_assets_dir};
use crate::materialize::svg::parse_icon_svg;

/// Materialize every in-scope `role: icon` / `role: decorative` vector entry.
pub fn materialize_icon_vectors(
    assets_dir: &Path, assets: &serde_json::Map<String, Value>, platforms: &[String],
    dry_run: bool, materialized: &mut Vec<Value>, skipped_pins: &mut Vec<Value>,
    errors: &mut Vec<Value>,
) {
    for (asset_id, entry) in assets {
        if !is_icon_vector_entry(entry) {
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
            let Some(layout) = export_layout(
                entry.get("role").and_then(Value::as_str).unwrap_or("icon"),
                "vector",
                platform,
                asset_id,
            ) else {
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
                Ok(written) => materialized.extend(written),
                Err(message) => errors.push(asset_error(asset_id, &message)),
            }
        }
    }
}

fn materialize_for_platform(
    tree: &Tree, asset_id: &str, platform: Platform, assets_dir: &Path, layout: &crate::materialize::paths::ExportLayout,
    dry_run: bool,
) -> Result<Vec<Value>, String> {
    let mut written = Vec::new();
    match platform {
        Platform::Ios => {
            let imageset_dir =
                resolve_under_assets_dir(assets_dir, &ios_imageset_dir(asset_id));
            if dry_run {
                for artifact in &layout.artifacts {
                    written.push(materialized_entry(asset_id, platform, artifact));
                }
                return Ok(written);
            }
            ios::write_imageset(tree, asset_id, &imageset_dir, dry_run).map_err(|err| {
                format!("asset `{asset_id}`: iOS export failed: {err}")
            })?;
            for artifact in &layout.artifacts {
                written.push(materialized_entry(asset_id, platform, artifact));
            }
        }
        Platform::Android => {
            let xml_rel = layout
                .pin
                .as_str();
            let xml_path = resolve_under_assets_dir(assets_dir, xml_rel);
            let drawable_name = xml_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(asset_id);
            if dry_run {
                for artifact in &layout.artifacts {
                    written.push(materialized_entry(asset_id, platform, artifact));
                }
                return Ok(written);
            }
            android::write_vector_drawable(tree, drawable_name, &xml_path).map_err(|err| {
                format!("asset `{asset_id}`: Android export failed: {err}")
            })?;
            for artifact in &layout.artifacts {
                written.push(materialized_entry(asset_id, platform, artifact));
            }
        }
    }
    Ok(written)
}

fn is_icon_vector_entry(entry: &Value) -> bool {
    let role = entry.get("role").and_then(Value::as_str);
    let kind = entry.get("kind").and_then(Value::as_str);
    matches!(role, Some("icon" | "decorative")) && kind == Some("vector")
}

fn active_platform_pin(entry: &Value, platform: &str, assets_dir: &Path) -> Option<String> {
    let pin = entry.get("sources")?.get(platform)?.as_str()?;
    let path = assets_dir.join(pin);
    if path.exists() { Some(pin.to_string()) } else { None }
}

fn materialized_entry(asset_id: &str, platform: Platform, path: &str) -> Value {
    json!({
        "asset_id": asset_id,
        "platform": platform.as_str(),
        "path": path,
    })
}

fn asset_error(asset_id: &str, message: &str) -> Value {
    json!({
        "path": format!("/assets/{asset_id}"),
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    const TRIANGLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="#010203" d="M12 2L2 22h20z"/>
</svg>"##;

    #[test]
    fn materialize_icon_writes_ios_and_android_exports() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(design.join("assets")).expect("assets dir");
        fs::write(design.join("assets/settings.svg"), TRIANGLE).expect("svg");

        let yaml = r#"version: 1
assets:
  settings:
    kind: vector
    role: icon
    alt: "Settings"
    source: assets/settings.svg
"#;
        fs::write(design.join("assets.yaml"), yaml).expect("yaml");

        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets map");

        let mut materialized = Vec::new();
        let mut skipped = Vec::new();
        let mut errors = Vec::new();
        materialize_icon_vectors(
            &design,
            assets,
            &["ios".into(), "android".into()],
            false,
            &mut materialized,
            &mut skipped,
            &mut errors,
        );
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let ios_pdf = design.join("assets/exports/ios/settings.imageset/settings.pdf");
        let ios_contents = design.join("assets/exports/ios/settings.imageset/Contents.json");
        let android_xml = design.join("assets/exports/android/drawable/settings.xml");

        assert!(ios_pdf.is_file() && ios_pdf.metadata().expect("meta").len() > 0);
        assert!(ios_contents.is_file() && ios_contents.metadata().expect("meta").len() > 0);
        assert!(android_xml.is_file() && android_xml.metadata().expect("meta").len() > 0);

        let xml = fs::read_to_string(android_xml).expect("xml");
        assert!(xml.contains("android:pathData"));
        assert_eq!(materialized.len(), 3);
    }

    #[test]
    fn dry_run_reports_without_writing_files() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(design.join("assets")).expect("assets dir");
        fs::write(design.join("assets/settings.svg"), TRIANGLE).expect("svg");

        let yaml = r#"version: 1
assets:
  settings:
    kind: vector
    role: icon
    alt: "Settings"
    source: assets/settings.svg
"#;
        let instance: Value = serde_saphyr::from_str(yaml).expect("parse yaml");
        let assets = instance.get("assets").and_then(Value::as_object).expect("assets map");

        let mut materialized = Vec::new();
        materialize_icon_vectors(
            &design,
            assets,
            &["ios".into()],
            true,
            &mut materialized,
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert!(!materialized.is_empty());
        assert!(!design.join("assets/exports/ios/settings.imageset").exists());
    }
}
