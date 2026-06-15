//! iOS PDF imageset export for icon vectors.

use std::path::Path;

use serde_json::json;
use usvg::Tree;

use super::pdf::write_icon_pdf;

/// Write an iOS imageset (`<id>.pdf` + `Contents.json`).
///
/// # Errors
///
/// Returns I/O errors from the underlying writes.
pub fn write_imageset(
    tree: &Tree, asset_id: &str, imageset_dir: &Path, dry_run: bool,
) -> std::io::Result<()> {
    let pdf_name = format!("{asset_id}.pdf");
    let pdf_path = imageset_dir.join(&pdf_name);
    let contents_path = imageset_dir.join("Contents.json");

    if dry_run {
        return Ok(());
    }

    std::fs::create_dir_all(imageset_dir)?;
    write_icon_pdf(tree, &pdf_path)?;
    let contents = json!({
        "images": [
            {
                "filename": pdf_name,
                "idiom": "universal"
            }
        ],
        "info": {
            "author": "vectis",
            "version": 1
        },
        "properties": {
            "preserves-vector-representation": true
        }
    });
    std::fs::write(contents_path, serde_json::to_vec_pretty(&contents).expect("contents json"))?;
    Ok(())
}
