//! Minimal vector PDF writer for iOS imageset exports.

use std::fmt::Write;

use usvg::tiny_skia_path::{PathSegment, Point};
use usvg::Tree;

use crate::materialize::svg::{collect_paths, DrawablePath};

/// Write a single-page vector PDF for an icon imageset.
///
/// # Errors
///
/// Returns I/O errors from the underlying write.
pub fn write_icon_pdf(tree: &Tree, out_path: &std::path::Path) -> std::io::Result<()> {
    let width = tree.size().width();
    let height = tree.size().height();
    let mut paths = Vec::new();
    collect_paths(tree.root(), &mut paths);
    let content = build_content_stream(&paths, height);
    let pdf = build_pdf(width, height, &content);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out_path, pdf)
}

fn build_content_stream(paths: &[DrawablePath], page_height: f32) -> String {
    let mut stream = String::new();
    for path in paths {
        let (r, g, b, _opacity) = path.color;
        let _ = writeln!(
            stream,
            "{} {} {} rg",
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0
        );
        append_pdf_path(&mut stream, &path.geometry, page_height);
        stream.push_str("f\n");
    }
    stream
}

fn append_pdf_path(out: &mut String, path: &usvg::tiny_skia_path::Path, page_height: f32) {
    let mut current = Point::from_xy(0.0, 0.0);
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                current = p;
                let _ = writeln!(out, "{} {} m", fmt(p.x), fmt(page_height - p.y));
            }
            PathSegment::LineTo(p) => {
                current = p;
                let _ = writeln!(out, "{} {} l", fmt(p.x), fmt(page_height - p.y));
            }
            PathSegment::QuadTo(control, end) => {
                let (c1, c2, end) = quad_to_cubic(current, control, end);
                current = end;
                let _ = writeln!(
                    out,
                    "{} {} {} {} {} {} c",
                    fmt(c1.x),
                    fmt(page_height - c1.y),
                    fmt(c2.x),
                    fmt(page_height - c2.y),
                    fmt(end.x),
                    fmt(page_height - end.y)
                );
            }
            PathSegment::CubicTo(p0, p1, p2) => {
                current = p2;
                let _ = writeln!(
                    out,
                    "{} {} {} {} {} {} c",
                    fmt(p0.x),
                    fmt(page_height - p0.y),
                    fmt(p1.x),
                    fmt(page_height - p1.y),
                    fmt(p2.x),
                    fmt(page_height - p2.y)
                );
            }
            PathSegment::Close => out.push_str("h\n"),
        }
    }
}

fn quad_to_cubic(start: Point, control: Point, end: Point) -> (Point, Point, Point) {
    let c1 = Point::from_xy(
        start.x + (2.0 / 3.0) * (control.x - start.x),
        start.y + (2.0 / 3.0) * (control.y - start.y),
    );
    let c2 = Point::from_xy(
        end.x + (2.0 / 3.0) * (control.x - end.x),
        end.y + (2.0 / 3.0) * (control.y - end.y),
    );
    (c1, c2, end)
}

fn fmt(value: f32) -> String {
    format!("{value:.4}").trim_end_matches('0').trim_end_matches('.').to_string()
}

fn build_pdf(width: f32, height: f32, content: &str) -> Vec<u8> {
    let mut body = String::new();
    let mut offsets = Vec::new();

    offsets.push(body.len());
    let _ = write!(
        body,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
    );

    offsets.push(body.len());
    let _ = write!(body, "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    offsets.push(body.len());
    let _ = write!(
        body,
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R /Resources << >> >>\nendobj\n"
    );

    offsets.push(body.len());
    let _ = write!(
        body,
        "4 0 obj\n<< /Length {} >>\nstream\n{content}endstream\nendobj\n",
        content.len()
    );

    let xref_offset = body.len();
    let object_count = offsets.len() + 1;
    let _ = write!(body, "xref\n0 {object_count}\n");
    let _ = writeln!(body, "0000000000 65535 f ");
    for offset in &offsets {
        let _ = writeln!(body, "{offset:010} 00000 n ");
    }
    let _ = write!(
        body,
        "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
    );

    body.into_bytes()
}
