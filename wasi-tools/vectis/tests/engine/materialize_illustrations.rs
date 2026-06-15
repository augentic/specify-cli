//! Illustration and photo materialize integration tests (RFC-46 R46-S18).

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

const TRIANGLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="#010203" d="M12 2L2 22h20z"/>
</svg>"##;

#[test]
fn materialize_illustration_vector_exports_exist() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/onboarding-hero.svg"), TRIANGLE).unwrap();

    let yaml = r#"version: 1
assets:
  onboarding-hero:
    kind: vector
    role: illustration
    alt: "Hero"
    source: assets/onboarding-hero.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize().args(["assets"]).arg(&assets_path).assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0));

    let ios_2x = design.join("assets/exports/ios/onboarding-hero.imageset/onboarding-hero@2x.png");
    let android_mdpi =
        design.join("assets/exports/android/drawable-mdpi/onboarding_hero.png");
    assert!(ios_2x.is_file() && android_mdpi.is_file());

    let img_2x = ImageReader::open(&ios_2x).unwrap().decode().unwrap();
    assert_eq!(img_2x.width(), 48);
    assert_eq!(img_2x.height(), 48);
}

#[test]
fn materialize_photo_copies_density_slots() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();

    let src = design.join("assets/hero@2x.png");
    let img = image::RgbaImage::from_pixel(48, 48, image::Rgba([9, 8, 7, 255]));
    img.save(&src).unwrap();

    let yaml = r#"version: 1
assets:
  hero:
    kind: raster
    role: photo
    alt: "Photo"
    sources:
      ios:
        2x: assets/hero@2x.png
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

    let export = design.join("assets/exports/ios/hero.imageset/hero@2x.png");
    assert!(export.is_file());
    assert_eq!(fs::read(export).unwrap(), fs::read(src).unwrap());
}
