//! Unit tests for shell catalog completeness probes.

use std::path::Path;

use tempfile::tempdir;

use super::*;

fn write_yaml(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir parents");
    }
    std::fs::write(path, content).expect("write yaml");
}

fn scaffold_project(root: &Path) {
    let specify = root.join(".specify");
    std::fs::create_dir_all(specify.join("specs")).expect("mkdir specs");
    write_yaml(
        &specify.join("project.yaml"),
        "name: test-app\nadapter: vectis\nspecify_version: '2.0'\nplatforms:\n  - core\n  - ios\n  - android\n",
    );
    let shared = root.join("shared/src");
    std::fs::create_dir_all(&shared).expect("mkdir shared");
    std::fs::write(shared.join("app.rs"), "pub struct App;").expect("write app.rs");
    let ios = root.join("iOS/TodoApp");
    std::fs::create_dir_all(ios.join("Resources/Assets.xcassets")).expect("mkdir ios");
    std::fs::write(ios.join("ContentView.swift"), "struct ContentView {}").expect("write swift");
    let android = root.join("Android/app/src/main/kotlin/com/test");
    std::fs::create_dir_all(&android).expect("mkdir android");
    std::fs::write(android.join("MainActivity.kt"), "class MainActivity").expect("write kt");
    std::fs::create_dir_all(root.join("Android/app/src/main/res")).expect("mkdir res");
}

fn write_inventory(root: &Path) {
    write_yaml(
        &root.join("design-system/assets.yaml"),
        r"
version: 1
assets:
  empty-tasks-hero:
    kind: vector
    role: illustration
    source: assets/empty-tasks-hero.svg
    sources:
      ios: assets/exports/ios/empty-tasks-hero.imageset/empty-tasks-hero@3x.png
      android: assets/exports/android/drawable-xxxhdpi/empty_tasks_hero.png
  chevron-right:
    kind: symbol
    role: icon
    symbols:
      ios: chevron.right
      android: chevron_right
",
    );
    write_yaml(
        &root.join(".specify/specs/composition.yaml"),
        r"
version: 1
screens:
  empty:
    body:
      - image:
          name: empty-tasks-hero
",
    );
}

#[test]
fn missing_ios_imageset_emits_finding() {
    let tmp = tempdir().unwrap();
    scaffold_project(tmp.path());
    write_inventory(tmp.path());

    let findings = catalog_findings(tmp.path(), &["ios".to_string(), "android".to_string()]);
    let ios_errors: Vec<_> = findings
        .iter()
        .filter(|f| f["id"] == "shell-catalog-entry-missing" && f["message"].as_str().unwrap().contains("ios"))
        .collect();
    assert_eq!(ios_errors.len(), 1);
    assert!(ios_errors[0]["message"].as_str().unwrap().contains("empty-tasks-hero"));
}

#[test]
fn contents_json_only_imageset_is_missing() {
    let tmp = tempdir().unwrap();
    scaffold_project(tmp.path());
    write_inventory(tmp.path());

    let imageset = tmp
        .path()
        .join("iOS/TodoApp/Resources/Assets.xcassets/empty-tasks-hero.imageset");
    std::fs::create_dir_all(&imageset).expect("mkdir imageset");
    std::fs::write(imageset.join("Contents.json"), "{\"images\":[]}").expect("write json");

    let findings = catalog_findings(tmp.path(), &["ios".to_string()]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["id"], "shell-catalog-entry-missing");
}

#[test]
fn present_shell_catalog_entries_emit_no_findings() {
    let tmp = tempdir().unwrap();
    scaffold_project(tmp.path());
    write_inventory(tmp.path());

    let imageset = tmp
        .path()
        .join("iOS/TodoApp/Resources/Assets.xcassets/empty-tasks-hero.imageset");
    std::fs::create_dir_all(&imageset).expect("mkdir imageset");
    std::fs::write(imageset.join("empty-tasks-hero@3x.png"), b"PNG").expect("write png");
    std::fs::write(imageset.join("Contents.json"), "{\"images\":[]}").expect("write json");

    let drawable = tmp
        .path()
        .join("Android/app/src/main/res/drawable-xxxhdpi/empty_tasks_hero.png");
    std::fs::create_dir_all(drawable.parent().unwrap()).expect("mkdir drawable");
    std::fs::write(&drawable, b"PNG").expect("write android png");

    let findings = catalog_findings(tmp.path(), &["ios".to_string(), "android".to_string()]);
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f["severity"] == "error")
        .collect();
    assert!(errors.is_empty(), "expected clean catalog: {findings:?}");
}

#[test]
fn symbol_references_are_skipped() {
    let tmp = tempdir().unwrap();
    scaffold_project(tmp.path());
    write_yaml(
        &tmp.path().join("design-system/assets.yaml"),
        r"
version: 1
assets:
  chevron-right:
    kind: symbol
    role: icon
    symbols:
      ios: chevron.right
      android: chevron_right
",
    );
    write_yaml(
        &tmp.path().join(".specify/specs/composition.yaml"),
        r"
version: 1
screens:
  list:
    body:
      - icon-button:
          icon: chevron-right
",
    );

    let findings = catalog_findings(tmp.path(), &["ios".to_string()]);
    assert!(findings.is_empty());
}

#[test]
fn vector_icon_android_requires_drawable_xml() {
    let tmp = tempdir().unwrap();
    scaffold_project(tmp.path());
    write_yaml(
        &tmp.path().join("design-system/assets.yaml"),
        r"
version: 1
assets:
  settings:
    kind: vector
    role: icon
    source: assets/settings.svg
",
    );
    write_yaml(
        &tmp.path().join(".specify/specs/composition.yaml"),
        r"
version: 1
screens:
  home:
    body:
      - icon-button:
          icon: settings
",
    );

    let findings = catalog_findings(tmp.path(), &["android".to_string()]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["id"], "shell-catalog-entry-missing");
    assert!(findings[0]["message"].as_str().unwrap().contains("drawable/settings.xml"));
}

#[test]
fn android_vector_icon_satisfied_by_drawable_xml() {
    let tmp = tempdir().unwrap();
    scaffold_project(tmp.path());
    write_yaml(
        &tmp.path().join("design-system/assets.yaml"),
        r"
version: 1
assets:
  settings:
    kind: vector
    role: icon
    source: assets/settings.svg
",
    );
    write_yaml(
        &tmp.path().join(".specify/specs/composition.yaml"),
        r"
version: 1
screens:
  home:
    body:
      - icon-button:
          icon: settings
",
    );
    let xml = tmp.path().join("Android/app/src/main/res/drawable/settings.xml");
    std::fs::create_dir_all(xml.parent().unwrap()).expect("mkdir drawable");
    std::fs::write(&xml, "<vector/>").expect("write xml");

    let findings = catalog_findings(tmp.path(), &["android".to_string()]);
    assert!(findings.is_empty());
}
