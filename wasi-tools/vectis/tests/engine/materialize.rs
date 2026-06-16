//! Materialize integration tests (RFC-46 Phase 2).

use std::fs;

use assert_cmd::Command;
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
fn materialize_icon_vector_exports_exist() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/chevron-right.svg"), TRIANGLE).unwrap();

    let yaml = r#"version: 1
assets:
  chevron-right:
    kind: vector
    role: icon
    alt: "Chevron"
    source: assets/chevron-right.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize().args(["assets"]).arg(&assets_path).assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0));

    let ios_pdf = design.join("assets/exports/ios/chevron-right.imageset/chevron-right.pdf");
    let android_xml = design.join("assets/exports/android/drawable/chevron_right.xml");
    assert!(ios_pdf.is_file() && ios_pdf.metadata().unwrap().len() > 0);
    assert!(android_xml.is_file() && android_xml.metadata().unwrap().len() > 0);
}

#[test]
fn materialize_icon_dry_run_skips_writes() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/chevron-right.svg"), TRIANGLE).unwrap();

    let yaml = r#"version: 1
assets:
  chevron-right:
    kind: vector
    role: icon
    alt: "Chevron"
    source: assets/chevron-right.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    let assert = vectis_materialize()
        .args(["assets", "--dry-run", "--platform", "ios"])
        .arg(&assets_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["materialized"].as_array().is_some_and(|arr| !arr.is_empty()));
    assert!(!design.join("assets/exports/ios/chevron-right.imageset").exists());
    assert_eq!(fs::read_to_string(&assets_path).unwrap(), yaml);
}

#[test]
fn materialize_auto_writes_sources_pins() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/chevron-right.svg"), TRIANGLE).unwrap();

    let yaml = r#"version: 1
assets:
  chevron-right:
    kind: vector
    role: icon
    alt: "Chevron"
    source: assets/chevron-right.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    vectis_materialize().args(["assets"]).arg(&assets_path).assert().success();

    let updated = fs::read_to_string(&assets_path).unwrap();
    assert!(updated.contains("sources:"));
    assert!(updated.contains("ios: assets/exports/ios/chevron-right.imageset/chevron-right.pdf"));
    assert!(updated.contains("android: assets/exports/android/drawable/chevron_right.xml"));
}

#[test]
fn materialize_skips_pinned_platform_despite_source_edit() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    let imageset = design.join("assets/exports/ios/chevron-right.imageset");
    fs::create_dir_all(&imageset).unwrap();
    fs::write(imageset.join("chevron-right.pdf"), b"%PDF-1.4 pinned").unwrap();
    fs::write(imageset.join("Contents.json"), r#"{"images":[],"info":{"version":1}}"#).unwrap();
    fs::write(design.join("assets/chevron-right.svg"), TRIANGLE).unwrap();

    let yaml = r#"version: 1
assets:
  chevron-right:
    kind: vector
    role: icon
    alt: "Chevron"
    source: assets/chevron-right.svg
    sources:
      ios: assets/exports/ios/chevron-right.imageset/chevron-right.pdf
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    fs::write(design.join("assets/chevron-right.svg"), r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="#ff0000" d="M2 2h20v20H2z"/>
</svg>"##)
    .unwrap();

    let assert = vectis_materialize()
        .args(["assets", "--platform", "ios"])
        .arg(&assets_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["materialized"].as_array().is_some_and(Vec::is_empty));
    assert!(value["skipped_pins"].as_array().is_some_and(|arr| !arr.is_empty()));
    assert_eq!(fs::read(imageset.join("chevron-right.pdf")).unwrap(), b"%PDF-1.4 pinned");
    assert_eq!(fs::read_to_string(&assets_path).unwrap(), yaml);
}

#[test]
fn materialize_second_run_is_noop() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    fs::create_dir_all(design.join("assets")).unwrap();
    fs::write(design.join("assets/chevron-right.svg"), TRIANGLE).unwrap();

    let yaml = r#"version: 1
assets:
  chevron-right:
    kind: vector
    role: icon
    alt: "Chevron"
    source: assets/chevron-right.svg
"#;
    let assets_path = design.join("assets.yaml");
    fs::write(&assets_path, yaml).unwrap();

    vectis_materialize().args(["assets"]).arg(&assets_path).assert().success();
    let after_first = fs::read_to_string(&assets_path).unwrap();

    let assert = vectis_materialize().args(["assets"]).arg(&assets_path).assert().success();
    let value = parse_json(&assert.get_output().stdout);
    assert!(value["materialized"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(fs::read_to_string(&assets_path).unwrap(), after_first);
}
