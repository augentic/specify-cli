//! Assets-mode integration tests. Exercises the embedded
//! `assets.schema.json` plus the cross-artifact resolution layer.

mod engine_support;

use std::path::PathBuf;

use engine_support::{
    errors_array, extract_envelope, warnings_array, write_assets_project, write_specs_composition,
};
use vectis_validate::error::VectisError;
use vectis_validate::{Args, ValidateMode, run};

/// Appendix E verbatim. Pinned here as the happy-path schema fixture
/// so any future drift surfaces first in this test.
const APPENDIX_E_ASSETS_YAML: &str = r#"version: 1

provenance:
  sources:
    - kind: manual

assets:
  empty-tasks-hero:
    kind: raster
    role: illustration
    alt: "Empty clipboard with a relaxed character beside it"
    sources:
      ios:
        1x: assets/empty-tasks-hero.png
        2x: assets/empty-tasks-hero@2x.png
        3x: assets/empty-tasks-hero@3x.png
      android:
        mdpi: assets/android/empty-tasks-hero-mdpi.png
        hdpi: assets/android/empty-tasks-hero-hdpi.png
        xhdpi: assets/android/empty-tasks-hero-xhdpi.png
        xxhdpi: assets/android/empty-tasks-hero-xxhdpi.png

  brand-logo:
    kind: vector
    role: illustration
    alt: "Acme logo"
    source: assets/brand-logo.svg
    sources:
      ios: assets/ios/brand-logo.pdf
      android: assets/android/brand-logo.xml

  settings:
    kind: symbol
    role: icon
    symbols:
      ios: gearshape
      android: settings
    tint: on-surface

  chevron-left:
    kind: symbol
    role: icon
    symbols:
      ios: chevron.left
      android: arrow_back
    tint: on-surface

  chevron-right:
    kind: symbol
    role: icon
    symbols:
      ios: chevron.right
      android: chevron_right
    tint: on-surface-variant

  plus:
    kind: symbol
    role: icon
    symbols:
      ios: plus
      android: add
    tint: on-primary
"#;

/// Files referenced by `APPENDIX_E_ASSETS_YAML`: every raster
/// density, the canonical SVG source, and both vector exports. Pinned
/// here so the happy-path test stays in lock-step with the fixture.
const APPENDIX_E_FILES: &[&str] = &[
    "assets/empty-tasks-hero.png",
    "assets/empty-tasks-hero@2x.png",
    "assets/empty-tasks-hero@3x.png",
    "assets/android/empty-tasks-hero-mdpi.png",
    "assets/android/empty-tasks-hero-hdpi.png",
    "assets/android/empty-tasks-hero-xhdpi.png",
    "assets/android/empty-tasks-hero-xxhdpi.png",
    "assets/brand-logo.svg",
    "assets/ios/brand-logo.pdf",
    "assets/android/brand-logo.xml",
];

/// Appendix E validates cleanly when paired with a composition that
/// references every asset id the manifest declares. With both ios and
/// android densities present (Appendix E's android side lacks
/// `xxxhdpi`, which surfaces as a warning, not an error), the run is
/// "errors-clean" rather than "absolutely silent".
#[test]
fn assets_appendix_e_paired_with_composition_validates_cleanly() {
    let (tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, APPENDIX_E_FILES);
    write_specs_composition(
        tmp.path(),
        // A trimmed composition that references the same asset ids as
        // Appendix C (icon-button, fab, image, icon items). Wiring-free
        // but already valid as a composition document for
        // reference-resolution purposes.
        r"version: 1
screens:
  task-list:
    name: Task list
    header:
      title: My tasks
      trailing:
        - icon-button:
            icon: settings
            label: Open settings
    body:
      list:
        each: tasks
        item:
          - group:
              direction: row
              items:
                - icon:
                    name: chevron-right
    fab:
      icon: plus
      label: Add task
    states:
      empty:
        when: tasks.is_empty
        replaces: body
        body:
          - group:
              direction: column
              items:
                - image:
                    name: empty-tasks-hero
  settings:
    name: Settings
    header:
      title: Settings
      leading:
        - icon-button:
            icon: chevron-left
            label: Back
    body:
      form: []
",
    );

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    assert_eq!(envelope["mode"], "assets");
    let errors = errors_array(&envelope);
    assert!(errors.is_empty(), "Appendix E + composition pairing unexpectedly errored: {errors:?}");
    // `xxxhdpi` is omitted on the android side of empty-tasks-hero,
    // so a warning is the expected shape -- not a failure.
    let warnings = warnings_array(&envelope);
    assert!(
        warnings
            .iter()
            .any(|w| w["message"].as_str().unwrap_or("").contains("missing optional `xxxhdpi`")),
        "expected at least one missing-density warning for xxxhdpi: {warnings:?}"
    );
}

/// A missing 1x raster file produces an error pointing at the asset
/// entry and the missing path. The `path` field uses the
/// JSON-Pointer-shaped indicator `/assets/<id>/sources/ios/1x`.
#[test]
fn assets_missing_raster_file_is_a_pathful_error() {
    let mut files = APPENDIX_E_FILES.to_vec();
    files.retain(|p| *p != "assets/empty-tasks-hero.png");
    let (_tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, &files);

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    let errors = errors_array(&envelope);
    let any_hits = errors.iter().any(|e| {
        e["path"].as_str().unwrap_or("") == "/assets/empty-tasks-hero/sources/ios/1x"
            && e["message"].as_str().unwrap_or("").contains("file not found")
    });
    assert!(any_hits, "expected a file-not-found error for 1x: {errors:?}");
}

/// Missing optional density is a warning, not an error. The fixture
/// trims the empty-tasks-hero raster down to just 2x and 3x on iOS
/// (and full android coverage) and crucially adds a sibling
/// composition that references the asset, because density warnings
/// only fire for composition-referenced assets.
#[test]
fn assets_missing_optional_density_is_a_warning() {
    let yaml = r"version: 1
assets:
  empty-tasks-hero:
    kind: raster
    role: illustration
    sources:
      ios:
        2x: assets/empty-tasks-hero@2x.png
        3x: assets/empty-tasks-hero@3x.png
      android:
        mdpi: assets/android/empty-tasks-hero-mdpi.png
        hdpi: assets/android/empty-tasks-hero-hdpi.png
        xhdpi: assets/android/empty-tasks-hero-xhdpi.png
        xxhdpi: assets/android/empty-tasks-hero-xxhdpi.png
        xxxhdpi: assets/android/empty-tasks-hero-xxxhdpi.png
";
    let files = [
        "assets/empty-tasks-hero@2x.png",
        "assets/empty-tasks-hero@3x.png",
        "assets/android/empty-tasks-hero-mdpi.png",
        "assets/android/empty-tasks-hero-hdpi.png",
        "assets/android/empty-tasks-hero-xhdpi.png",
        "assets/android/empty-tasks-hero-xxhdpi.png",
        "assets/android/empty-tasks-hero-xxxhdpi.png",
    ];
    let (tmp, assets_path) = write_assets_project(yaml, &files);
    write_specs_composition(
        tmp.path(),
        r"version: 1
screens:
  s:
    name: S
    body:
      list:
        item:
          - image:
              name: empty-tasks-hero
",
    );

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    assert!(errors_array(&envelope).is_empty(), "errors unexpected: {envelope}");
    let warnings = warnings_array(&envelope);
    assert!(
        warnings
            .iter()
            .any(|w| w["message"].as_str().unwrap_or("").contains("missing optional `1x`")),
        "expected a missing-1x warning, got: {warnings:?}"
    );
}

/// Composition referencing an asset id that is NOT in `assets.yaml`
/// is an error.
#[test]
fn assets_unresolved_composition_reference_is_an_error() {
    let (tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, APPENDIX_E_FILES);
    write_specs_composition(
        tmp.path(),
        // `mystery-glyph` is not in Appendix E.
        r"version: 1
screens:
  s:
    name: S
    header:
      title: T
      trailing:
        - icon-button:
            icon: mystery-glyph
            label: Mystery
    body:
      list:
        item: []
",
    );

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("references unknown asset id `mystery-glyph`")),
        "expected unresolved-reference error, got: {errors:?}"
    );
}

/// Vector asset referenced by composition but missing
/// `sources.android` is an error (the targeted shell platform has no
/// usable source).
#[test]
fn assets_vector_missing_platform_export_is_an_error() {
    let yaml = r"version: 1
assets:
  brand-logo:
    kind: vector
    role: illustration
    source: assets/brand-logo.svg
    sources:
      ios: assets/ios/brand-logo.pdf
";
    let files = ["assets/brand-logo.svg", "assets/ios/brand-logo.pdf"];
    let (tmp, assets_path) = write_assets_project(yaml, &files);
    write_specs_composition(
        tmp.path(),
        r"version: 1
screens:
  s:
    name: S
    body:
      list:
        item:
          - image:
              name: brand-logo
",
    );

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| {
            e["path"].as_str().unwrap_or("") == "/assets/brand-logo/sources/android"
                && e["message"].as_str().unwrap_or("").contains("vector asset `brand-logo`")
        }),
        "expected android-coverage error, got: {errors:?}"
    );
}

/// When NO sibling composition is found, density warnings and
/// platform-coverage errors do not fire -- only schema and
/// file-existence checks. (The raster below has only ios sources --
/// valid at the schema layer because `sources.minProperties: 1` -- and
/// is fine without composition reference.)
#[test]
fn assets_without_sibling_composition_only_runs_schema_and_files() {
    let yaml = r"version: 1
assets:
  empty-tasks-hero:
    kind: raster
    role: illustration
    sources:
      ios:
        2x: assets/empty-tasks-hero@2x.png
";
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/empty-tasks-hero@2x.png"]);

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    assert!(errors_array(&envelope).is_empty(), "no errors expected: {envelope}");
    assert!(
        warnings_array(&envelope).is_empty(),
        "no warnings expected without composition: {envelope}"
    );
}

#[test]
fn assets_missing_file_returns_invalid_project_error() {
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(PathBuf::from("/definitely/not/here/assets.yaml")),
    };
    match run(&args) {
        Err(VectisError::InvalidProject { message }) => {
            assert!(message.contains("assets.yaml not readable"), "unexpected message: {message}");
        }
        other => panic!("expected InvalidProject for missing file, got {other:?}"),
    }
}

/// Schema rejection still fires for assets-mode (e.g. invalid `kind`);
/// the rejection rides the same envelope shape as the cross-artifact
/// errors and the dispatcher exits non-zero.
#[test]
fn assets_schema_violation_reports_pathful_error() {
    let yaml = r"version: 1
assets:
  bad:
    kind: raster
    role: photograph
    sources:
      ios:
        1x: assets/bad.png
";
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/bad.png"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["path"].as_str().unwrap_or("").contains("/assets/bad")),
        "expected a schema error pointing at /assets/bad: {errors:?}"
    );
}

/// Asset id case violation (uppercase letter) is rejected by the
/// schema's `propertyNames` pattern and surfaces as an error rooted at
/// the assets map.
#[test]
fn assets_kebab_case_violation_is_a_schema_error() {
    let yaml = r"version: 1
assets:
  Bad-Id:
    kind: symbol
    role: icon
    symbols:
      ios: foo
";
    let (_tmp, assets_path) = write_assets_project(yaml, &[]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = extract_envelope(run(&args).expect("run succeeds"));
    assert!(
        !errors_array(&envelope).is_empty(),
        "expected at least one schema error for `Bad-Id`: {envelope}"
    );
}
