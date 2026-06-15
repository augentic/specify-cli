//! Android Vector Drawable export for icon vectors.

use std::fmt::Write;

use usvg::Tree;

use crate::materialize::svg::{collect_paths, path_data_string};

/// Write a `drawable/<id>.xml` Vector Drawable for an icon.
///
/// # Errors
///
/// Returns I/O errors from the underlying write.
pub fn write_vector_drawable(
    tree: &Tree, _drawable_name: &str, out_path: &std::path::Path,
) -> std::io::Result<()> {
    let width = tree.size().width();
    let height = tree.size().height();
    let mut paths = Vec::new();
    collect_paths(tree.root(), &mut paths);

    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    let _ = write!(
        body,
        "<vector xmlns:android=\"http://schemas.android.com/apk/res/android\"\n    android:width=\"{width}dp\"\n    android:height=\"{height}dp\"\n    android:viewportWidth=\"{width}\"\n    android:viewportHeight=\"{height}\">\n"
    );

    for drawable in paths {
        let path_data = path_data_string(&drawable.geometry);
        if path_data.is_empty() {
            continue;
        }
        let (r, g, b, opacity) = drawable.color;
        let _ = write!(
            body,
            "    <path\n        android:fillColor=\"{color}\"\n        android:fillAlpha=\"{opacity}\"\n        android:pathData=\"{path_data}\"/>\n",
            color = android_color(r, g, b),
            opacity = trim_num(opacity)
        );
    }

    body.push_str("</vector>\n");

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out_path, body)
}

fn android_color(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn trim_num(value: f32) -> String {
    format!("{value:.4}").trim_end_matches('0').trim_end_matches('.').to_string()
}
