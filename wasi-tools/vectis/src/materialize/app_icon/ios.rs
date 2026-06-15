//! iOS `AppIcon.appiconset` export for auto-converted app icons (RFC-46 §4.2).

use std::io::Cursor;
use std::path::Path;

use image::RgbaImage;
use serde_json::json;

pub const APPICON_PNG_NAME: &str = "AppIcon.png";

/// Write a single-size iOS 11+ `AppIcon.appiconset` from a 1024×1024 canvas.
///
/// # Errors
///
/// Returns a human-readable message when directory creation, PNG encoding, or
/// JSON serialization fails.
pub fn write_appiconset(canvas: &RgbaImage, appiconset_dir: &Path) -> Result<(), String> {
    if canvas.dimensions() != (1024, 1024) {
        return Err(format!(
            "internal: app-icon canvas must be 1024×1024 (got {}×{})",
            canvas.width(),
            canvas.height()
        ));
    }

    std::fs::create_dir_all(appiconset_dir).map_err(|err| {
        format!(
            "AppIcon.appiconset write failed at {}: {err}",
            appiconset_dir.display()
        )
    })?;

    let png_path = appiconset_dir.join(APPICON_PNG_NAME);
    let mut png_bytes = Vec::new();
    canvas
        .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|err| format!("AppIcon.png encode failed: {err}"))?;
    std::fs::write(&png_path, png_bytes).map_err(|err| {
        format!("AppIcon.png write failed at {}: {err}", png_path.display())
    })?;

    let contents = json!({
        "images": [
            {
                "filename": APPICON_PNG_NAME,
                "idiom": "universal",
                "platform": "ios",
                "size": "1024x1024"
            }
        ],
        "info": {
            "author": "xcode",
            "version": 1
        }
    });
    let contents_path = appiconset_dir.join("Contents.json");
    std::fs::write(
        &contents_path,
        serde_json::to_vec_pretty(&contents).expect("contents json"),
    )
    .map_err(|err| format!("Contents.json write failed at {}: {err}", contents_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use image::Rgba;
    use serde_json::Value;
    use tempfile::tempdir;

    #[test]
    fn appiconset_writes_actool_friendly_layout() {
        let tmp = tempdir().expect("tempdir");
        let dir = tmp.path().join("AppIcon.appiconset");
        let canvas = RgbaImage::from_pixel(1024, 1024, Rgba([10, 20, 30, 255]));

        write_appiconset(&canvas, &dir).expect("write");

        let png = dir.join(APPICON_PNG_NAME);
        let contents = dir.join("Contents.json");
        assert!(png.is_file() && png.metadata().expect("meta").len() > 0);
        assert!(contents.is_file());

        let parsed: Value =
            serde_json::from_slice(&fs::read(&contents).expect("read")).expect("json");
        let images = parsed["images"].as_array().expect("images array");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0]["filename"], APPICON_PNG_NAME);
        assert_eq!(images[0]["idiom"], "universal");
        assert_eq!(images[0]["platform"], "ios");
        assert_eq!(images[0]["size"], "1024x1024");

        let decoded = image::ImageReader::open(&png).expect("open").decode().expect("decode");
        assert_eq!(decoded.width(), 1024);
        assert_eq!(decoded.height(), 1024);
    }
}
