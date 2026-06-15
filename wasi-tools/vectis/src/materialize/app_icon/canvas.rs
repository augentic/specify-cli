//! Shared 1024×1024 launcher canvas decode for `role: app-icon` (RFC-46 §4.1).

use std::path::Path;

use image::{DynamicImage, ImageReader, RgbaImage};
use usvg::Tree;

use crate::materialize::render::render_tree_to_png;

/// Fixed launcher canvas edge length (iOS path A and Android path A share this).
pub const LAUNCHER_CANVAS_SIZE: u32 = 1024;

/// Decode an app-icon `source:` master into a normalized 1024×1024 RGBA canvas.
///
/// Raster masters must be square with width and height ≥1024 (no upscale), and
/// must not carry alpha for iOS auto-convert. Larger square masters are
/// downscaled to 1024×1024.
///
/// # Errors
///
/// Returns `assets-app-icon-source-invalid: …` when the master cannot be decoded
/// or violates path-A constraints.
pub fn decode_to_launcher_canvas(
    source_path: &Path, source_rel: &str, asset_id: &str,
) -> Result<RgbaImage, String> {
    let ext = source_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    match ext.as_deref() {
        Some("svg") => decode_svg_canvas(source_path, source_rel, asset_id),
        Some("png" | "jpg" | "jpeg" | "webp") => {
            decode_raster_canvas(source_path, source_rel, asset_id)
        }
        _ => Err(format!(
            "assets-app-icon-source-invalid: app-icon `{asset_id}` `source:` `{source_rel}` has no recognised master extension"
        )),
    }
}

fn decode_svg_canvas(
    source_path: &Path, source_rel: &str, asset_id: &str,
) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(source_path).map_err(|err| {
        format!(
            "assets-app-icon-source-invalid: app-icon `{asset_id}` `source:` `{source_rel}` not readable: {err}"
        )
    })?;
    let tree = Tree::from_data(&bytes, &usvg::Options::default()).map_err(|err| {
        format!(
            "assets-app-icon-source-invalid: app-icon `{asset_id}` SVG decode failed: {err}"
        )
    })?;
    let png = render_tree_to_png(&tree, LAUNCHER_CANVAS_SIZE, LAUNCHER_CANVAS_SIZE).map_err(
        |err| {
            format!(
                "assets-app-icon-source-invalid: app-icon `{asset_id}` SVG rasterize failed: {err}"
            )
        },
    )?;
    let image = image::load_from_memory(&png).map_err(|err| {
        format!(
            "assets-app-icon-source-invalid: app-icon `{asset_id}` SVG rasterize failed: {err}"
        )
    })?;
    Ok(flatten_to_opaque(image.to_rgba8()))
}

fn decode_raster_canvas(
    source_path: &Path, source_rel: &str, asset_id: &str,
) -> Result<RgbaImage, String> {
    let image = ImageReader::open(source_path)
        .map_err(|err| {
            format!(
                "assets-app-icon-source-invalid: app-icon `{asset_id}` `source:` `{source_rel}` not readable: {err}"
            )
        })?
        .with_guessed_format()
        .map_err(|err| {
            format!(
                "assets-app-icon-source-invalid: app-icon `{asset_id}` raster decode failed: {err}"
            )
        })?
        .decode()
        .map_err(|err| {
            format!(
                "assets-app-icon-source-invalid: app-icon `{asset_id}` raster decode failed: {err}"
            )
        })?;

    let (width, height) = (image.width(), image.height());
    if width != height {
        return Err(format!(
            "assets-app-icon-source-invalid: raster app-icon `{asset_id}` master must be square (got {width}×{height})"
        ));
    }
    if width < LAUNCHER_CANVAS_SIZE {
        return Err(format!(
            "assets-app-icon-source-invalid: raster app-icon `{asset_id}` master must be at least 1024×1024 (got {width}×{height})"
        ));
    }
    if raster_has_alpha(&image) {
        return Err(format!(
            "assets-app-icon-source-invalid: raster app-icon `{asset_id}` master must be opaque for iOS auto-convert (image has alpha)"
        ));
    }

    let rgba = if width > LAUNCHER_CANVAS_SIZE {
        image
            .resize_exact(
                LAUNCHER_CANVAS_SIZE,
                LAUNCHER_CANVAS_SIZE,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8()
    } else {
        image.to_rgba8()
    };
    Ok(flatten_to_opaque(rgba))
}

fn raster_has_alpha(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) => false,
        DynamicImage::ImageLumaA8(_) | DynamicImage::ImageRgba8(_) => image
            .to_rgba8()
            .pixels()
            .any(|pixel| pixel[3] < 255),
        _ => image.to_rgba8().pixels().any(|pixel| pixel[3] < 255),
    }
}

/// Composite any residual transparency onto an opaque white background.
fn flatten_to_opaque(mut canvas: RgbaImage) -> RgbaImage {
    for pixel in canvas.pixels_mut() {
        if pixel[3] < 255 {
            let alpha = f32::from(pixel[3]) / 255.0;
            pixel[0] = blend_channel(pixel[0], alpha);
            pixel[1] = blend_channel(pixel[1], alpha);
            pixel[2] = blend_channel(pixel[2], alpha);
            pixel[3] = 255;
        }
    }
    canvas
}

fn blend_channel(foreground: u8, alpha: f32) -> u8 {
    let blended = (f32::from(foreground) * alpha) + (255.0 * (1.0 - alpha));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "blended channel is clamped to 0..=255 before narrowing"
    )]
    {
        blended.round().clamp(0.0, 255.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use image::{ImageFormat, Rgba, RgbaImage};
    use tempfile::tempdir;

    const SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <rect width="1024" height="1024" fill="#336699"/>
</svg>"##;

    #[test]
    fn svg_decodes_to_1024_canvas() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("app-icon.svg");
        fs::write(&path, SQUARE_SVG).expect("write svg");

        let canvas = decode_to_launcher_canvas(&path, "assets/app-icon.svg", "app-icon")
            .expect("decode svg");
        assert_eq!(canvas.dimensions(), (1024, 1024));
        assert!(canvas.pixels().all(|px| px[3] == 255));
    }

    #[test]
    fn raster_1024_decodes_without_upscale() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("app-icon.png");
        let img = RgbaImage::from_pixel(1024, 1024, Rgba([4, 5, 6, 255]));
        img.save_with_format(&path, ImageFormat::Png).expect("write png");

        let canvas =
            decode_to_launcher_canvas(&path, "assets/app-icon.png", "app-icon").expect("decode");
        assert_eq!(canvas.dimensions(), (1024, 1024));
        assert_eq!(canvas.get_pixel(0, 0).0, [4, 5, 6, 255]);
    }

    #[test]
    fn raster_below_1024_rejects() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("small.png");
        let img = RgbaImage::from_pixel(512, 512, Rgba([1, 2, 3, 255]));
        img.save_with_format(&path, ImageFormat::Png).expect("write png");

        let err = decode_to_launcher_canvas(&path, "assets/small.png", "app-icon").unwrap_err();
        assert!(err.contains("assets-app-icon-source-invalid"));
        assert!(err.contains("512"));
    }

    #[test]
    fn raster_alpha_rejects() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("alpha.png");
        let img = RgbaImage::from_pixel(1024, 1024, Rgba([1, 2, 3, 128]));
        img.save_with_format(&path, ImageFormat::Png).expect("write png");

        let err = decode_to_launcher_canvas(&path, "assets/alpha.png", "app-icon").unwrap_err();
        assert!(err.contains("assets-app-icon-source-invalid"));
        assert!(err.contains("opaque"));
    }
}
