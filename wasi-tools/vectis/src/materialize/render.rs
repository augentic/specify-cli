//! SVG rasterization via `resvg` for illustration exports.

use resvg::tiny_skia::{Pixmap, Transform};
use usvg::Tree;

/// Render a parsed SVG tree to PNG bytes at the given pixel dimensions.
///
/// # Errors
///
/// Returns a human-readable message when allocation or PNG encoding fails.
pub fn render_tree_to_png(tree: &Tree, out_width: u32, out_height: u32) -> Result<Vec<u8>, String> {
    if out_width == 0 || out_height == 0 {
        return Err("render dimensions must be non-zero".into());
    }

    let mut pixmap = Pixmap::new(out_width, out_height)
        .ok_or_else(|| "render buffer allocation failed".to_string())?;

    let svg_size = tree.size();
    let scale_x = f64::from(out_width) / f64::from(svg_size.width());
    let scale_y = f64::from(out_height) / f64::from(svg_size.height());
    #[expect(
        clippy::cast_possible_truncation,
        reason = "scale ratios for designer-scale SVGs are far below f32 max"
    )]
    let transform = Transform::from_scale(scale_x as f32, scale_y as f32);

    resvg::render(tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .map_err(|err| format!("PNG encode failed: {err}"))
}

/// Pixel dimensions for a 1× logical SVG canvas scaled by `factor`.
#[must_use]
pub fn scaled_dimensions(tree: &Tree, factor: f32) -> (u32, u32) {
    let size = tree.size();
    (pixel_dim(size.width(), factor), pixel_dim(size.height(), factor))
}

fn pixel_dim(logical: f32, factor: f32) -> u32 {
    let scaled = (logical * factor).round().max(1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "SVG logical dimensions are designer-scale; products fit comfortably in u32"
    )]
    {
        scaled as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materialize::svg::parse_icon_svg;

    const TRIANGLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="#010203" d="M12 2L2 22h20z"/>
</svg>"##;

    #[test]
    fn render_produces_png_with_expected_dimensions() {
        let parsed = parse_icon_svg(TRIANGLE.as_bytes(), "tri").expect("parse");
        let (w, h) = scaled_dimensions(&parsed.tree, 2.0);
        assert_eq!((w, h), (48, 48));

        let png = render_tree_to_png(&parsed.tree, w, h).expect("render");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.len() > 64);
    }

    #[test]
    fn deterministic_output_for_fixed_input() {
        let parsed = parse_icon_svg(TRIANGLE.as_bytes(), "tri").expect("parse");
        let (w, h) = scaled_dimensions(&parsed.tree, 3.0);
        let first = render_tree_to_png(&parsed.tree, w, h).expect("first");
        let second = render_tree_to_png(&parsed.tree, w, h).expect("second");
        assert_eq!(first, second);
    }
}
