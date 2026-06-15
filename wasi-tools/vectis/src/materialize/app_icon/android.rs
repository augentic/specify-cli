//! Android adaptive + legacy mipmap export for auto-converted app icons (RFC-46 §4.3).

use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, Rgba, RgbaImage, imageops::FilterType};
use serde_json::Value;

use crate::materialize::paths::{ANDROID_DENSITIES, android_density_factor};

const ADAPTIVE_CANVAS_DP: f32 = 108.0;
const SAFE_ZONE_RATIO: f32 = 66.0 / ADAPTIVE_CANVAS_DP;
const LEGACY_LAUNCHER_DP: f32 = 48.0;
const DEFAULT_BACKGROUND: &str = "#FFFFFF";

const IC_LAUNCHER_XML: &str = include_str!("../../../../../templates/vectis/android/ic_launcher.xml");
const IC_LAUNCHER_ROUND_XML: &str =
    include_str!("../../../../../templates/vectis/android/ic_launcher_round.xml");

/// Write the adaptive + legacy mipmap tree under an Android app-icon export root.
///
/// # Errors
///
/// Returns a human-readable message when directory creation or file writes fail.
pub fn write_android_export(
    canvas: &RgbaImage, background_hex: &str, export_root: &Path,
) -> Result<(), String> {
    if canvas.dimensions() != (1024, 1024) {
        return Err(format!(
            "internal: app-icon canvas must be 1024×1024 (got {}×{})",
            canvas.width(),
            canvas.height()
        ));
    }

    std::fs::create_dir_all(export_root).map_err(|err| {
        format!(
            "Android app-icon export failed at {}: {err}",
            export_root.display()
        )
    })?;

    write_xml(
        &export_root.join("mipmap-anydpi-v26/ic_launcher.xml"),
        IC_LAUNCHER_XML,
    )?;
    write_xml(
        &export_root.join("mipmap-anydpi-v26/ic_launcher_round.xml"),
        IC_LAUNCHER_ROUND_XML,
    )?;
    write_background_xml(&export_root.join("values/ic_launcher_background.xml"), background_hex)?;

    for density in ANDROID_DENSITIES {
        let factor = android_density_factor(density).unwrap_or(1.0);
        let fg_size = scaled_dp(ADAPTIVE_CANVAS_DP, factor);
        let fg = compose_foreground(canvas, fg_size);
        write_png(
            &export_root.join(format!("drawable-{density}/ic_launcher_foreground.png")),
            &fg,
        )?;

        let legacy_size = scaled_dp(LEGACY_LAUNCHER_DP, factor);
        let bg = parse_hex_color(background_hex).unwrap_or([255, 255, 255]);
        let legacy = compose_legacy(canvas, legacy_size, bg);
        write_png(
            &export_root.join(format!("mipmap-{density}/ic_launcher.png")),
            &legacy,
        )?;
    }

    Ok(())
}

/// Resolve the adaptive-icon background colour from `tint` + sibling `tokens.yaml`.
#[must_use]
pub fn resolve_launcher_background(entry: &Value, assets_dir: &Path) -> String {
    let Some(tint_name) = entry.get("tint").and_then(Value::as_str) else {
        return DEFAULT_BACKGROUND.to_string();
    };
    let tokens_path = assets_dir.join("tokens.yaml");
    let Ok(source) = std::fs::read_to_string(&tokens_path) else {
        return DEFAULT_BACKGROUND.to_string();
    };
    let Ok(tokens) = serde_saphyr::from_str::<Value>(&source) else {
        return DEFAULT_BACKGROUND.to_string();
    };
    tokens
        .get("colors")
        .and_then(|colors| colors.get(tint_name))
        .and_then(|color| color.get("light").and_then(Value::as_str))
        .filter(|hex| parse_hex_color(hex).is_some())
        .map_or_else(|| DEFAULT_BACKGROUND.to_string(), ToString::to_string)
}

fn compose_foreground(canvas: &RgbaImage, output_size: u32) -> RgbaImage {
    scale_centered_on_transparent(canvas, output_size)
}

fn compose_legacy(canvas: &RgbaImage, output_size: u32, bg: [u8; 3]) -> RgbaImage {
    let mut out =
        RgbaImage::from_pixel(output_size, output_size, Rgba([bg[0], bg[1], bg[2], 255]));
    let fg = scale_centered_on_transparent(canvas, output_size);
    image::imageops::overlay(&mut out, &fg, 0, 0);
    out
}

fn scale_centered_on_transparent(canvas: &RgbaImage, output_size: u32) -> RgbaImage {
    let safe = safe_zone_pixels(output_size);
    let scaled = if canvas.width() == safe {
        canvas.clone()
    } else {
        image::imageops::resize(canvas, safe, safe, FilterType::Lanczos3)
    };
    let mut out = RgbaImage::from_pixel(output_size, output_size, Rgba([0, 0, 0, 0]));
    let x = i64::from((output_size - safe) / 2);
    let y = i64::from((output_size - safe) / 2);
    image::imageops::overlay(&mut out, &scaled, x, y);
    out
}

fn safe_zone_pixels(output_size: u32) -> u32 {
    let scaled = f64::from(output_size) * f64::from(SAFE_ZONE_RATIO);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "launcher canvas sizes are small designer-scale dp values"
    )]
    {
        scaled.round().max(1.0) as u32
    }
}

fn scaled_dp(dp: f32, factor: f32) -> u32 {
    let scaled = (dp * factor).round().max(1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Android launcher dp sizes fit comfortably in u32"
    )]
    {
        scaled as u32
    }
}

fn write_background_xml(path: &Path, background_hex: &str) -> Result<(), String> {
    let hex = parse_hex_color(background_hex).map_or_else(
        || DEFAULT_BACKGROUND.to_string(),
        |rgb| format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]),
    );
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">{hex}</color>
</resources>
"#
    );
    write_xml(path, &body)
}

fn write_xml(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!("Android app-icon mkdir failed at {}: {err}", parent.display())
        })?;
    }
    std::fs::write(path, body.as_bytes()).map_err(|err| {
        format!("Android app-icon write failed at {}: {err}", path.display())
    })
}

fn write_png(path: &Path, image: &RgbaImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!("Android app-icon mkdir failed at {}: {err}", parent.display())
        })?;
    }
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|err| format!("Android app-icon PNG encode failed: {err}"))?;
    std::fs::write(path, bytes).map_err(|err| {
        format!("Android app-icon write failed at {}: {err}", path.display())
    })
}

fn parse_hex_color(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use image::Rgba;
    use tempfile::tempdir;

    #[test]
    fn adaptive_xml_is_well_formed() {
        assert!(IC_LAUNCHER_XML.contains("<adaptive-icon"));
        assert!(IC_LAUNCHER_XML.contains("@drawable/ic_launcher_foreground"));
        assert!(IC_LAUNCHER_ROUND_XML.contains("<adaptive-icon"));
    }

    #[test]
    fn resolve_background_uses_tint_token_light_variant() {
        let tmp = tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        fs::create_dir_all(&design).expect("mkdir");
        fs::write(
            design.join("tokens.yaml"),
            "version: 1\ncolors:\n  brand:\n    light: \"#AABBCC\"\n    dark: \"#001122\"\n",
        )
        .expect("tokens");

        let entry = serde_json::json!({ "tint": "brand" });
        assert_eq!(resolve_launcher_background(&entry, &design), "#AABBCC");
    }

    #[test]
    fn resolve_background_defaults_without_tint() {
        let tmp = tempdir().expect("tempdir");
        let entry = serde_json::json!({});
        assert_eq!(
            resolve_launcher_background(&entry, tmp.path()),
            DEFAULT_BACKGROUND
        );
    }

    #[test]
    fn write_android_export_creates_required_tree() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("app-icon");
        let canvas = RgbaImage::from_pixel(1024, 1024, Rgba([20, 40, 60, 255]));

        write_android_export(&canvas, "#112233", &root).expect("write");

        for rel in [
            "mipmap-anydpi-v26/ic_launcher.xml",
            "mipmap-anydpi-v26/ic_launcher_round.xml",
            "values/ic_launcher_background.xml",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }

        for density in ANDROID_DENSITIES {
            assert!(
                root.join(format!("drawable-{density}/ic_launcher_foreground.png")).is_file(),
                "missing foreground {density}"
            );
            assert!(
                root.join(format!("mipmap-{density}/ic_launcher.png")).is_file(),
                "missing legacy {density}"
            );
        }

        let bg = fs::read_to_string(root.join("values/ic_launcher_background.xml")).expect("read");
        assert!(bg.contains("ic_launcher_background"));
        assert!(bg.contains("#112233"));

        let launcher = fs::read_to_string(root.join("mipmap-anydpi-v26/ic_launcher.xml"))
            .expect("read launcher xml");
        assert!(launcher.contains("adaptive-icon"));
    }
}
