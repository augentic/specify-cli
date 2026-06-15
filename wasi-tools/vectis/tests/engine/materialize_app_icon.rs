//! App-icon materialize integration tests (RFC-46 R46-S19 / R46-S20).

use std::fs;

use assert_cmd::Command;
use image::ImageReader;
use serde_json::Value;
use tempfile::tempdir;

fn vectis_materialize() -> Command {
    let mut cmd = Command::cargo_bin("vectis").expect("vectis binary");
    cmd.arg("materialize");
    cmd
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("json output")
}

const SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <rect width="1024" height="1024" fill="#445566"/>
</svg>"##;

#[test]
fn materialize_app_icon_ios_exports_exist() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/app-icon.svg"), SQUARE_SVG).unwrap();

    let yaml = r#"version: 1
app-icon: app-icon
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize()
        .args(["assets", "--platform", "ios"])
        .arg(&assets_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0));

    let appiconset = design.join("assets/exports/ios/app-icon/AppIcon.appiconset");
    let png = appiconset.join("AppIcon.png");
    let contents = appiconset.join("Contents.json");
    assert!(png.is_file() && contents.is_file());

    let parsed: Value = serde_json::from_slice(&fs::read(&contents).unwrap()).unwrap();
    assert!(parsed.get("images").and_then(Value::as_array).is_some_and(|a| !a.is_empty()));

    let img = ImageReader::open(&png).unwrap().decode().unwrap();
    assert_eq!(img.width(), 1024);
    assert_eq!(img.height(), 1024);

    let updated = fs::read_to_string(&assets_path).unwrap();
    assert!(updated.contains("sources:"));
    assert!(updated.contains("ios: assets/exports/ios/app-icon/AppIcon.appiconset"));
}

#[test]
fn materialize_app_icon_android_exports_exist() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/app-icon.svg"), SQUARE_SVG).unwrap();

    let yaml = r#"version: 1
app-icon: app-icon
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize()
        .args(["assets", "--platform", "android"])
        .arg(&assets_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0));

    let root = design.join("assets/exports/android/app-icon");
    assert!(root.join("mipmap-anydpi-v26/ic_launcher.xml").is_file());
    assert!(root.join("mipmap-anydpi-v26/ic_launcher_round.xml").is_file());
    assert!(root.join("values/ic_launcher_background.xml").is_file());
    assert!(root.join("drawable-xxxhdpi/ic_launcher_foreground.png").is_file());
    assert!(root.join("mipmap-xxxhdpi/ic_launcher.png").is_file());

    let launcher = fs::read_to_string(root.join("mipmap-anydpi-v26/ic_launcher.xml")).unwrap();
    assert!(launcher.contains("adaptive-icon"));
    assert!(launcher.contains("ic_launcher_foreground"));

    let background =
        fs::read_to_string(root.join("values/ic_launcher_background.xml")).unwrap();
    assert!(background.contains("ic_launcher_background"));
}

#[test]
fn materialize_app_icon_ios_rejects_small_raster() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();

    let png_path = design.join("assets/app-icon.png");
    let img = image::RgbaImage::from_pixel(512, 512, image::Rgba([1, 2, 3, 255]));
    img.save(&png_path).unwrap();

    let yaml = r#"version: 1
assets:
  app-icon:
    kind: raster
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.png
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize()
        .args(["assets", "--platform", "ios"])
        .arg(&assets_path)
        .assert()
        .failure();
    let value = parse_json(&assert.get_output().stdout);
    let errors = value["errors"].as_array().expect("errors");
    assert!(errors.iter().any(|entry| {
        entry["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-app-icon-source-invalid")
    }));
}

#[test]
fn materialize_app_icon_android_rejects_small_raster() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();

    let png_path = design.join("assets/app-icon.png");
    let img = image::RgbaImage::from_pixel(512, 512, image::Rgba([1, 2, 3, 255]));
    img.save(&png_path).unwrap();

    let yaml = r#"version: 1
assets:
  app-icon:
    kind: raster
    role: app-icon
    alt: "App icon"
    source: assets/app-icon.png
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize()
        .args(["assets", "--platform", "android"])
        .arg(&assets_path)
        .assert()
        .failure();
    let value = parse_json(&assert.get_output().stdout);
    let errors = value["errors"].as_array().expect("errors");
    assert!(errors.iter().any(|entry| {
        entry["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-app-icon-source-invalid")
    }));
}
