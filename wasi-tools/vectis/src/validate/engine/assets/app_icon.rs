//! `role: app-icon` structural validation (RFC-46 §4.2 / §4.3).

use std::path::Path;

use serde_json::{Value, json};

/// Cross-check `sources.ios` / `sources.android` path suffixes and
/// raster `source:` master constraints.
pub(super) fn check_app_icon_entry(id: &str, entry: &Value, errors: &mut Vec<Value>) {
    if entry.get("role").and_then(Value::as_str) != Some("app-icon") {
        return;
    }
    check_ios_source_suffix(id, entry, errors);
    if let Some(source) = entry.get("source").and_then(Value::as_str) {
        check_source_kind_alignment(id, entry, source, errors);
    }
}

/// Validate a pinned iOS / Android export tree when the pin resolves.
pub(super) fn check_export_layout(
    json_path: &str, path_rel: &str, platform: &str, dir: &Path, errors: &mut Vec<Value>,
) {
    let resolved = dir.join(path_rel);
    if !resolved.is_dir() {
        return;
    }
    match platform {
        "ios" => check_ios_appiconset(json_path, &resolved, errors),
        "android" => check_android_export(json_path, &resolved, errors),
        _ => {}
    }
}

/// Decode raster `source:` dimensions / alpha for path A (RFC §4.1).
pub(super) fn check_raster_source_master(
    id: &str, source_rel: &str, assets_dir: &Path, errors: &mut Vec<Value>,
) {
    let path = assets_dir.join(source_rel);
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if ext.as_deref() != Some("png") {
        return;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Some((width, height, has_alpha)) = png_ihdr(&bytes) else {
        errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-source-invalid: raster app-icon `{id}` `source:` `{source_rel}` is not a decodable PNG"
            ),
        }));
        return;
    };
    if width != height {
        errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-source-invalid: raster app-icon `{id}` master must be square (got {width}×{height})"
            ),
        }));
    } else if width < 1024 {
        errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-source-invalid: raster app-icon `{id}` master must be at least 1024×1024 (got {width}×{height})"
            ),
        }));
    }
    if has_alpha {
        errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-source-invalid: raster app-icon `{id}` master must be opaque for iOS auto-convert (PNG has alpha)"
            ),
        }));
    }
}

fn check_ios_source_suffix(id: &str, entry: &Value, errors: &mut Vec<Value>) {
    let Some(ios) = entry.get("sources").and_then(|s| s.get("ios")).and_then(Value::as_str) else {
        return;
    };
    if source_extension(ios).as_deref() == Some("svg") {
        errors.push(json!({
            "path": format!("/assets/{id}/sources/ios"),
            "message": format!(
                "assets-app-icon-export-invalid: app-icon `{id}` `sources.ios` must not end in `.svg` (use PDF export or AppIcon.appiconset pin)"
            ),
        }));
    }
}

fn check_source_kind_alignment(id: &str, entry: &Value, source: &str, errors: &mut Vec<Value>) {
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    let ext = source_extension(source);
    match (kind, ext.as_deref()) {
        ("vector", Some("svg")) | ("raster", Some("png" | "jpg" | "jpeg" | "webp")) => {}
        ("vector", _) => errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-kind-source-mismatch: vector app-icon `{id}` requires `source:` with a `.svg` extension, got `{source}`"
            ),
        })),
        ("raster", Some(ext)) => errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-kind-source-mismatch: raster app-icon `{id}` `source:` extension `.{ext}` is not an allowed master format (png, jpg, jpeg, webp)"
            ),
        })),
        ("raster", None) => errors.push(json!({
            "path": format!("/assets/{id}/source"),
            "message": format!(
                "assets-app-icon-source-invalid: raster app-icon `{id}` `source:` `{source}` has no recognised raster extension"
            ),
        })),
        _ => errors.push(json!({
            "path": format!("/assets/{id}/kind"),
            "message": format!(
                "assets-app-icon-kind-source-mismatch: app-icon `{id}` `kind` `{kind}` disagrees with `source:` extension"
            ),
        })),
    }
}

fn check_ios_appiconset(json_path: &str, root: &Path, errors: &mut Vec<Value>) {
    if directory_contains_extension(root, "svg") {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: AppIcon.appiconset must not contain raw SVG files",
        }));
    }
    let contents = root.join("Contents.json");
    if !contents.is_file() {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: AppIcon.appiconset is missing `Contents.json`",
        }));
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&contents) else {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: AppIcon.appiconset `Contents.json` is not readable",
        }));
        return;
    };
    if serde_json::from_str::<Value>(&raw).is_err() {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: AppIcon.appiconset `Contents.json` is not valid JSON",
        }));
    }
    if !directory_contains_extension(root, "png") {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: AppIcon.appiconset must contain at least one PNG entry",
        }));
    }
}

fn check_android_export(json_path: &str, root: &Path, errors: &mut Vec<Value>) {
    let required = ["mipmap-anydpi-v26/ic_launcher.xml", "mipmap-anydpi-v26/ic_launcher_round.xml"];
    for rel in required {
        if !root.join(rel).is_file() {
            errors.push(json!({
                "path": json_path,
                "message": format!(
                    "assets-app-icon-export-invalid: Android app-icon export is missing `{rel}`"
                ),
            }));
        }
    }
    let foreground = root.join("drawable/ic_launcher_foreground.xml").is_file()
        || android_foreground_png_exists(root);
    if !foreground {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: Android app-icon export is missing `drawable/ic_launcher_foreground.xml` or density foreground PNGs",
        }));
    }
    let background = root.join("values/ic_launcher_background.xml").is_file()
        || root.join("values/colors.xml").is_file();
    if !background {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: Android app-icon export is missing `values/ic_launcher_background.xml` or `values/colors.xml`",
        }));
    }
    let legacy = ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"]
        .iter()
        .any(|density| root.join(format!("mipmap-{density}/ic_launcher.png")).is_file());
    if !legacy {
        errors.push(json!({
            "path": json_path,
            "message": "assets-app-icon-export-invalid: Android app-icon export is missing legacy `mipmap-*/ic_launcher.png` fallback",
        }));
    }
}

fn android_foreground_png_exists(root: &Path) -> bool {
    for density in ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"] {
        if root.join(format!("drawable-{density}/ic_launcher_foreground.png")).is_file() {
            return true;
        }
    }
    false
}

fn directory_contains_extension(dir: &Path, ext: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
    })
}

/// Lowercase extension without the leading dot, or `None` when absent.
fn source_extension(path: &str) -> Option<String> {
    Path::new(path).extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase)
}

/// Read PNG IHDR width, height, and whether the color type carries alpha.
fn png_ihdr(bytes: &[u8]) -> Option<(u32, u32, bool)> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 26 || !bytes.starts_with(SIG) {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    let color_type = bytes[25];
    let has_alpha = matches!(color_type, 4 | 6);
    Some((width, height, has_alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_ihdr_reads_square_opaque() {
        let png = minimal_png(1024, 1024, 2);
        let (w, h, alpha) = png_ihdr(&png).expect("ihdr");
        assert_eq!(w, 1024);
        assert_eq!(h, 1024);
        assert!(!alpha);
    }

    fn minimal_png(width: u32, height: u32, color_type: u8) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(color_type);
        ihdr.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace
        append_chunk(&mut out, *b"IHDR", &ihdr);
        append_chunk(&mut out, *b"IEND", &[]);
        out
    }

    fn append_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
        let len = u32::try_from(data.len()).expect("png fixture chunk length fits u32");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&kind);
        out.extend_from_slice(data);
        let crc = crc32(kind, data);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    fn crc32(kind: [u8; 4], data: &[u8]) -> u32 {
        let mut hasher = 0xffff_ffff_u32;
        for byte in kind.iter().chain(data) {
            hasher ^= u32::from(*byte);
            for _ in 0..8 {
                hasher = if hasher & 1 == 1 { 0xedb8_8320 ^ (hasher >> 1) } else { hasher >> 1 };
            }
        }
        !hasher
    }
}
