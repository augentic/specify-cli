//! SVG load and lightweight profile checks for icon materialization (RFC-46 §2).

use std::fmt::Write;

use usvg::tiny_skia_path::{Path, PathSegment};
use usvg::{Node, Paint, Tree};

/// Parsed SVG ready for platform export.
#[derive(Debug)]
pub struct ParsedSvg {
    pub tree: Tree,
}

/// Load and validate an SVG master for icon vector export.
///
/// # Errors
///
/// Returns a human-readable message naming the asset when the SVG uses
/// unsupported features (gradients, text, filters, embedded images, …).
pub fn parse_icon_svg(svg_bytes: &[u8], asset_id: &str) -> Result<ParsedSvg, String> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_bytes, &opt)
        .map_err(|err| format!("asset `{asset_id}`: SVG parse failed: {err}"))?;

    validate_profile(&tree, asset_id)?;
    if !tree_has_drawable_paths(tree.root()) {
        return Err(format!("asset `{asset_id}`: SVG contains no drawable paths"));
    }

    Ok(ParsedSvg { tree })
}

fn validate_profile(tree: &Tree, asset_id: &str) -> Result<(), String> {
    if tree.has_text_nodes() {
        return Err(format!("asset `{asset_id}`: text nodes are not supported"));
    }
    if tree.has_defs_nodes() {
        return Err(format!(
            "asset `{asset_id}`: gradients, patterns, clip paths, masks, or filters are not supported"
        ));
    }
    walk_profile(tree.root(), asset_id)
}

fn walk_profile(group: &usvg::Group, asset_id: &str) -> Result<(), String> {
    if group.opacity().get() < 1.0 {
        return Err(format!("asset `{asset_id}`: group opacity is not supported"));
    }
    if group.blend_mode() != usvg::BlendMode::Normal {
        return Err(format!("asset `{asset_id}`: non-normal blend modes are not supported"));
    }
    if group.clip_path().is_some() || group.mask().is_some() || !group.filters().is_empty() {
        return Err(format!(
            "asset `{asset_id}`: clip paths, masks, and filters are not supported"
        ));
    }

    for child in group.children() {
        match child {
            Node::Group(nested) => walk_profile(nested, asset_id)?,
            Node::Path(path) => validate_path(path, asset_id)?,
            Node::Image(_) => {
                return Err(format!("asset `{asset_id}`: embedded raster images are not supported"));
            }
            Node::Text(_) => {
                return Err(format!("asset `{asset_id}`: text nodes are not supported"));
            }
        }
    }
    Ok(())
}

fn validate_path(path: &usvg::Path, asset_id: &str) -> Result<(), String> {
    if !path.is_visible() {
        return Ok(());
    }
    if let Some(fill) = path.fill() {
        ensure_solid_paint(fill.paint(), asset_id, "fill")?;
    }
    if let Some(stroke) = path.stroke() {
        ensure_solid_paint(stroke.paint(), asset_id, "stroke")?;
    }
    if path.fill().is_none() && path.stroke().is_none() {
        return Err(format!("asset `{asset_id}`: path has no fill or stroke"));
    }
    Ok(())
}

fn ensure_solid_paint(paint: &Paint, asset_id: &str, kind: &str) -> Result<(), String> {
    match paint {
        Paint::Color(_) => Ok(()),
        Paint::LinearGradient(_) | Paint::RadialGradient(_) | Paint::Pattern(_) => {
            Err(format!("asset `{asset_id}`: {kind} gradients and patterns are not supported"))
        }
    }
}

fn tree_has_drawable_paths(group: &usvg::Group) -> bool {
    group.children().iter().any(|node| match node {
        Node::Group(nested) => tree_has_drawable_paths(nested),
        Node::Path(path) => path.is_visible(),
        Node::Image(_) | Node::Text(_) => false,
    })
}

/// Absolute canvas-space path data for a `usvg` path node.
///
/// # Errors
///
/// Returns `None` when the transform cannot be applied to the geometry.
#[must_use]
pub fn absolute_path(path: &usvg::Path) -> Option<Path> {
    path.data().clone().transform(path.abs_transform())
}

/// Format path segments as Android `pathData` (SVG `d` syntax).
#[must_use]
pub fn path_data_string(path: &Path) -> String {
    let mut out = String::new();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                append_coord(&mut out, 'M', p.x, p.y);
            }
            PathSegment::LineTo(p) => {
                append_coord(&mut out, 'L', p.x, p.y);
            }
            PathSegment::QuadTo(p0, p1) => {
                let _ = write!(
                    out,
                    "Q{},{},{},{} ",
                    trim_num(p0.x),
                    trim_num(p0.y),
                    trim_num(p1.x),
                    trim_num(p1.y)
                );
            }
            PathSegment::CubicTo(p0, p1, p2) => {
                let _ = write!(
                    out,
                    "C{},{},{},{},{},{} ",
                    trim_num(p0.x),
                    trim_num(p0.y),
                    trim_num(p1.x),
                    trim_num(p1.y),
                    trim_num(p2.x),
                    trim_num(p2.y)
                );
            }
            PathSegment::Close => out.push('Z'),
        }
    }
    out.trim().to_string()
}

fn append_coord(out: &mut String, verb: char, x: f32, y: f32) {
    let _ = write!(out, "{verb}{} {} ", trim_num(x), trim_num(y));
}

fn trim_num(value: f32) -> String {
    let rounded = format!("{value:.4}");
    rounded.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Resolve a solid RGBA fill for PDF / Android export.
#[must_use]
pub fn path_fill_rgba(path: &usvg::Path) -> Option<(u8, u8, u8, f32)> {
    if let Some(fill) = path.fill()
        && let Paint::Color(color) = fill.paint()
    {
        return Some((color.red, color.green, color.blue, fill.opacity().get()));
    }
    if let Some(stroke) = path.stroke()
        && let Paint::Color(color) = stroke.paint()
    {
        return Some((color.red, color.green, color.blue, stroke.opacity().get()));
    }
    None
}

/// Collect drawable paths in paint order for export backends.
pub fn collect_paths(group: &usvg::Group, out: &mut Vec<DrawablePath>) {
    for child in group.children() {
        match child {
            Node::Group(nested) => collect_paths(nested, out),
            Node::Path(path) => {
                if !path.is_visible() {
                    continue;
                }
                if let Some(geometry) = absolute_path(path)
                    && let Some(color) = path_fill_rgba(path)
                {
                    out.push(DrawablePath { geometry, color });
                }
            }
            Node::Image(_) | Node::Text(_) => {}
        }
    }
}

/// Canvas-space path plus solid fill colour.
#[derive(Debug, Clone)]
pub struct DrawablePath {
    pub geometry: Path,
    pub color: (u8, u8, u8, f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="#010203" d="M12 2L2 22h20z"/>
</svg>"##;

    #[test]
    fn parse_simple_icon_succeeds() {
        let parsed = parse_icon_svg(TRIANGLE.as_bytes(), "settings").expect("parse");
        assert!(parsed.tree.size().width() > 0.0);
    }

    #[test]
    fn path_data_uses_space_separated_coords() {
        let parsed = parse_icon_svg(TRIANGLE.as_bytes(), "tri").expect("parse");
        let mut paths = Vec::new();
        collect_paths(parsed.tree.root(), &mut paths);
        let path_data = path_data_string(&paths[0].geometry);
        assert_eq!(path_data, "M12 2 L2 22 L22 22 Z");
    }

    #[test]
    fn filter_defs_are_rejected() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <filter id="blur"><feGaussianBlur stdDeviation="2"/></filter>
  <rect width="24" height="24" filter="url(#blur)"/>
</svg>"#;
        let err = parse_icon_svg(svg.as_bytes(), "bad").unwrap_err();
        assert!(err.contains("bad"));
        assert!(err.contains("filters"));
    }
}
