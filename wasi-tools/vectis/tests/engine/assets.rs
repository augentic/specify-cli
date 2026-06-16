//! Assets-mode integration tests. Exercises the embedded
//! `assets.schema.json` plus the cross-artifact resolution layer.

use std::path::PathBuf;

use specify_vectis::validate::error::VectisError;
use specify_vectis::validate::{ValidateArgs as Args, ValidateMode, run};

use crate::engine_support::{
    errors_array, warnings_array, write_assets_project, write_project_yaml, write_specs_composition,
};

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
fn appendix_e_with_composition_validates() {
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
    let envelope = run(&args).expect("run succeeds");
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
fn missing_raster_file_pathful() {
    let mut files = APPENDIX_E_FILES.to_vec();
    files.retain(|p| *p != "assets/empty-tasks-hero.png");
    let (_tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, &files);

    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
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
fn missing_optional_density_warns() {
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
    let envelope = run(&args).expect("run succeeds");
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
fn unresolved_composition_reference_errors() {
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
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("references unknown asset id `mystery-glyph`")),
        "expected unresolved-reference error, got: {errors:?}"
    );
}

/// Vector asset referenced by composition with only a canonical
/// `source:` does not require a per-platform pin yet (RFC §7 path A).
#[test]
fn vector_source_only_satisfies_platform_coverage() {
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
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
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
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        !errors
            .iter()
            .any(|e| { e["path"].as_str().unwrap_or("") == "/assets/brand-logo/sources/android" }),
        "canonical `source:` should satisfy android coverage until materialize runs: {errors:?}"
    );
}

/// When NO sibling composition is found, density warnings and
/// platform-coverage errors do not fire -- only schema and
/// file-existence checks. (The raster below has only ios sources --
/// valid at the schema layer because `sources.minProperties: 1` -- and
/// is fine without composition reference.)
#[test]
fn without_sibling_composition_runs_schema_and_files() {
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
    let envelope = run(&args).expect("run succeeds");
    assert!(errors_array(&envelope).is_empty(), "no errors expected: {envelope}");
    assert!(
        warnings_array(&envelope).is_empty(),
        "no warnings expected without composition: {envelope}"
    );
}

#[test]
fn missing_file_invalid_project() {
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
fn schema_violation_pathful() {
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
    let envelope = run(&args).expect("run succeeds");
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
fn kebab_case_violation_schema_error() {
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
    let envelope = run(&args).expect("run succeeds");
    assert!(
        !errors_array(&envelope).is_empty(),
        "expected at least one schema error for `Bad-Id`: {envelope}"
    );
}

/// A vector entry with both `variant_of` and `usage_hint` validates
/// without errors.
#[test]
fn variant_of_and_usage_hint_validate() {
    let yaml = r#"version: 1
assets:
  nav-lists-active:
    kind: vector
    role: icon
    variant_of: nav-lists
    usage_hint: "Outlined shapes with faint circular background halo"
    source: assets/nav-lists-active.svg
"#;
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/nav-lists-active.svg"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "variant_of + usage_hint should validate cleanly: {envelope}"
    );
}

/// A `variant_of` value that violates the kebab-case pattern is
/// rejected by the schema.
#[test]
fn variant_of_pattern_violation_schema_error() {
    let yaml = "version: 1
assets:
  nav-lists-active:
    kind: vector
    role: icon
    variant_of: Nav-Lists
    source: assets/nav-lists-active.svg
";
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/nav-lists-active.svg"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        !errors_array(&envelope).is_empty(),
        "expected schema error for non-kebab variant_of `Nav-Lists`: {envelope}"
    );
}

/// All three entry kinds (raster, vector, symbol) accept `variant_of`
/// and `usage_hint`. A single fixture with one of each validates
/// cleanly.
#[test]
fn variant_of_on_each_entry_kind() {
    let yaml = r#"version: 1
assets:
  hero-active:
    kind: raster
    role: illustration
    variant_of: hero
    usage_hint: "Active state hero illustration"
    sources:
      ios:
        1x: assets/hero-active.png

  logo-active:
    kind: vector
    role: icon
    variant_of: logo
    usage_hint: "Active state logo"
    source: assets/logo-active.svg

  gear-active:
    kind: symbol
    role: icon
    variant_of: gear
    usage_hint: "Active state gear icon"
    symbols:
      ios: gearshape.fill
"#;
    let files = ["assets/hero-active.png", "assets/logo-active.svg"];
    let (_tmp, assets_path) = write_assets_project(yaml, &files);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "variant_of on all three entry kinds should validate cleanly: {envelope}"
    );
}

/// RFC §3.1 path A — vector `app-icon` with SVG master validates.
#[test]
fn app_icon_vector_master_validates() {
    let yaml = r#"version: 1
app-icon: app-icon
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "Application"
    source: assets/app-icon.svg
"#;
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/app-icon.svg"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "vector app-icon master should validate cleanly: {envelope}"
    );
}

/// RFC §3.1 path A — raster `app-icon` with PNG master validates.
#[test]
fn app_icon_raster_master_validates() {
    let yaml = r#"version: 1
app-icon: app-icon-png
assets:
  app-icon-png:
    kind: raster
    role: app-icon
    alt: "Application"
    source: assets/app-icon.png
"#;
    let (tmp, assets_path) = write_assets_project(yaml, &[]);
    let png_path = tmp.path().join("design-system/assets/app-icon.png");
    std::fs::write(png_path, minimal_png_bytes(1024, 1024, 2)).expect("write app-icon png");
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "raster app-icon master should validate cleanly: {envelope}"
    );
}

/// RFC §3.1 path B — operator-pinned export roots validate when layout satisfies §4.2 / §4.3.
#[test]
fn app_icon_pinned_export_roots_validates() {
    let yaml = r#"version: 1
app-icon: app-icon-pinned
assets:
  app-icon-pinned:
    kind: raster
    role: app-icon
    alt: "Application"
    sources:
      ios: assets/exports/ios/app-icon/AppIcon.appiconset
      android: assets/exports/android/app-icon
"#;
    let (tmp, assets_path) = write_assets_project(yaml, &[]);
    let design = tmp.path().join("design-system");
    write_valid_ios_appiconset(&design.join("assets/exports/ios/app-icon/AppIcon.appiconset"));
    write_valid_android_app_icon_export(&design.join("assets/exports/android/app-icon"));
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "pinned app-icon export roots should validate cleanly: {envelope}"
    );
}

/// Empty AppIcon.appiconset directories fail export layout checks.
#[test]
fn app_icon_invalid_ios_export_errors() {
    let yaml = r#"version: 1
app-icon: app-icon-pinned
assets:
  app-icon-pinned:
    kind: raster
    role: app-icon
    alt: "Application"
    sources:
      ios: assets/exports/ios/app-icon/AppIcon.appiconset
"#;
    let (tmp, assets_path) = write_assets_project(yaml, &[]);
    std::fs::create_dir_all(
        tmp.path().join("design-system/assets/exports/ios/app-icon/AppIcon.appiconset"),
    )
    .expect("mkdir empty appiconset");
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-app-icon-export-invalid")),
        "expected export layout error, got: {errors:?}"
    );
}

/// `sources.ios` ending in `.svg` is rejected for `role: app-icon`.
#[test]
fn app_icon_ios_svg_pin_errors() {
    let yaml = r#"version: 1
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "Application"
    sources:
      ios: assets/ios/app-icon.svg
"#;
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/ios/app-icon.svg"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-app-icon-export-invalid")),
        "expected ios svg pin error, got: {errors:?}"
    );
}

/// Illustration vector `sources.ios` ending in `.svg` surfaces a warning.
#[test]
fn illustration_ios_svg_source_warns() {
    let yaml = r"version: 1
assets:
  brand-logo:
    kind: vector
    role: illustration
    source: assets/brand-logo.svg
    sources:
      ios: assets/ios/brand-logo.svg
      android: assets/android/brand-logo.xml
";
    let files =
        ["assets/brand-logo.svg", "assets/ios/brand-logo.svg", "assets/android/brand-logo.xml"];
    let (_tmp, assets_path) = write_assets_project(yaml, &files);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    let warnings = warnings_array(&envelope);
    assert!(
        warnings.iter().any(|w| w["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-svg-illustration-on-ios")),
        "expected ios svg illustration warning, got: {warnings:?}"
    );
}

/// Composition-referenced raster without pins or exports is materialization-missing.
#[test]
fn raster_materialization_missing_errors() {
    let yaml = r"version: 1
assets:
  hero:
    kind: raster
    role: illustration
    sources:
      ios:
        1x: assets/hero.png
";
    let (tmp, assets_path) = write_assets_project(yaml, &["assets/hero.png"]);
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
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
              name: hero
",
    );
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-materialization-missing")
            && e["path"].as_str().unwrap_or("").contains("/sources/android")),
        "expected android materialization-missing error, got: {errors:?}"
    );
}

/// Composition-referenced vector without pins, exports, or `source:` is
/// materialization-missing — not `assets-app-icon-invalid`.
#[test]
fn vector_materialization_missing_errors() {
    let yaml = r"version: 1
assets:
  brand-logo:
    kind: vector
    role: illustration
    sources:
      ios: assets/ios/brand-logo.pdf
";
    let (tmp, assets_path) = write_assets_project(yaml, &["assets/ios/brand-logo.pdf"]);
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
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
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-materialization-missing")
            && e["path"].as_str().unwrap_or("").contains("/sources/android")
            && !e["message"].as_str().unwrap_or("").contains("assets-app-icon-invalid")),
        "expected android materialization-missing error, got: {errors:?}"
    );
}

/// Committed exports under the conventional materialize layout satisfy
/// platform coverage without per-platform pins.
#[test]
fn committed_exports_satisfy_platform_coverage() {
    let yaml = r"version: 1
assets:
  brand-logo:
    kind: vector
    role: illustration
    sources:
      ios: assets/ios/brand-logo.pdf
";
    let (tmp, assets_path) = write_assets_project(yaml, &["assets/ios/brand-logo.pdf"]);
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(design.join("assets/exports/android/drawable-mdpi")).expect("mkdir");
    std::fs::write(design.join("assets/exports/android/drawable-mdpi/brand_logo.png"), b"PNG")
        .expect("write png");
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
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
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "ios pin plus committed android export should satisfy both platforms: {envelope}"
    );
}

/// `materialize assets` followed by `validate assets` leaves a clean
/// envelope for a composition-referenced vector icon.
#[test]
fn materialize_then_validate_passes() {
    use specify_vectis::materialize::{AssetsArgs, MaterializeCommand, run as materialize_run};

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 2L2 22h20z"/></svg>"#;
    let yaml = r"version: 1
assets:
  chevron-right:
    kind: vector
    role: icon
    source: assets/chevron-right.svg
";
    let (tmp, assets_path) = write_assets_project(yaml, &[]);
    let design = tmp.path().join("design-system");
    std::fs::write(design.join("assets/chevron-right.svg"), SVG).expect("write svg");
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
    write_specs_composition(
        tmp.path(),
        r"version: 1
screens:
  s:
    name: S
    body:
      list:
        item:
          - icon:
              name: chevron-right
",
    );

    let before = run(&Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path.clone()),
    })
    .expect("pre-materialize validate");
    assert!(
        errors_array(&before).is_empty(),
        "path A `source:` satisfies coverage before materialize: {before}"
    );

    materialize_run(&MaterializeCommand::Assets(AssetsArgs {
        path: Some(assets_path.clone()),
        platform: None,
        dry_run: false,
    }))
    .expect("materialize succeeds");

    assert!(
        design
            .join("assets/exports/ios/chevron-right.imageset/chevron-right.pdf")
            .is_file(),
        "ios export should exist after materialize"
    );

    let after = run(&Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    })
    .expect("post-materialize validate");
    assert!(
        errors_array(&after).is_empty(),
        "validate should pass after materialize wrote committed exports: {after}"
    );
}

/// `project.yaml` with only `ios` does not require android coverage.
#[test]
fn platforms_from_project_yaml_ios_only() {
    let yaml = r"version: 1
assets:
  hero:
    kind: raster
    role: illustration
    sources:
      ios:
        1x: assets/hero.png
";
    let (tmp, assets_path) = write_assets_project(yaml, &["assets/hero.png"]);
    write_project_yaml(tmp.path(), &["core", "ios"]);
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
              name: hero
",
    );
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "ios-only project should not require android sources: {envelope}"
    );
}

fn write_valid_ios_appiconset(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("mkdir appiconset");
    std::fs::write(
        dir.join("Contents.json"),
        r#"{"images":[{"filename":"AppIcon.png","idiom":"universal","platform":"ios","size":"1024x1024"}],"info":{"version":1,"author":"xcode"}}"#,
    )
    .expect("write Contents.json");
    std::fs::write(dir.join("AppIcon.png"), minimal_png_bytes(1024, 1024, 2)).expect("write png");
}

fn write_valid_android_app_icon_export(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("mipmap-anydpi-v26")).expect("mkdir anydpi");
    std::fs::create_dir_all(root.join("drawable")).expect("mkdir drawable");
    std::fs::create_dir_all(root.join("values")).expect("mkdir values");
    for density in ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"] {
        let dir = root.join(format!("mipmap-{density}"));
        std::fs::create_dir_all(&dir).expect("mkdir mipmap");
        std::fs::write(dir.join("ic_launcher.png"), minimal_png_bytes(48, 48, 2))
            .expect("write launcher png");
    }
    std::fs::write(root.join("mipmap-anydpi-v26/ic_launcher.xml"), "<adaptive-icon/>")
        .expect("write ic_launcher.xml");
    std::fs::write(root.join("mipmap-anydpi-v26/ic_launcher_round.xml"), "<adaptive-icon/>")
        .expect("write ic_launcher_round.xml");
    std::fs::write(root.join("drawable/ic_launcher_foreground.xml"), "<vector/>")
        .expect("write foreground");
    std::fs::write(root.join("values/ic_launcher_background.xml"), "<resources/>")
        .expect("write background");
}

fn minimal_png_bytes(width: u32, height: u32, color_type: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8);
    ihdr.push(color_type);
    ihdr.extend_from_slice(&[0, 0, 0]);
    append_png_chunk(&mut out, *b"IHDR", &ihdr);
    append_png_chunk(&mut out, *b"IEND", &[]);
    out
}

fn append_png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).expect("png fixture chunk length fits u32");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let crc = png_crc(kind, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn png_crc(kind: [u8; 4], data: &[u8]) -> u32 {
    let mut hasher = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        hasher ^= u32::from(*byte);
        for _ in 0..8 {
            hasher = if hasher & 1 == 1 { 0xedb8_8320 ^ (hasher >> 1) } else { hasher >> 1 };
        }
    }
    !hasher
}

/// `source:` on a non-app-icon raster entry is rejected by the schema.
#[test]
fn raster_icon_with_source_schema_rejects() {
    let yaml = r"version: 1
assets:
  bad-icon:
    kind: raster
    role: icon
    source: assets/bad-icon.png
    sources:
      ios:
        1x: assets/bad-icon.png
";
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/bad-icon.png"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        !errors_array(&envelope).is_empty(),
        "expected schema rejection for `source:` on role: icon raster: {envelope}"
    );
}

/// Vector `kind` with a raster `source:` extension is a cross-check error.
#[test]
fn app_icon_kind_source_mismatch_errors() {
    let yaml = r#"version: 1
assets:
  app-icon:
    kind: vector
    role: app-icon
    alt: "Application"
    source: assets/app-icon.png
"#;
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/app-icon.png"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("assets-app-icon-kind-source-mismatch")),
        "expected kind/source mismatch error, got: {errors:?}"
    );
}

/// Top-level `app-icon` pointer must reference an existing `role: app-icon` entry.
#[test]
fn app_icon_pointer_cross_check_errors() {
    let yaml = r"version: 1
app-icon: missing
assets:
  settings:
    kind: symbol
    role: icon
    symbols:
      ios: gearshape
";
    let (_tmp, assets_path) = write_assets_project(yaml, &[]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(
        errors
            .iter()
            .any(|e| e["message"].as_str().unwrap_or("").contains("assets-app-icon-invalid")),
        "expected app-icon pointer error, got: {errors:?}"
    );
}

/// `inferred: true` on a symbol entry is schema-valid.
#[test]
fn symbol_inferred_validates() {
    let yaml = r"version: 1
assets:
  chevron-right:
    kind: symbol
    role: icon
    inferred: true
    symbols:
      ios: chevron.right
      android: chevron_right
";
    let (_tmp, assets_path) = write_assets_project(yaml, &[]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "inferred symbol should validate cleanly: {envelope}"
    );
}

/// A `usage_hint` without `variant_of` is schema-valid (the fields
/// are independently optional).
#[test]
fn usage_hint_alone_validates() {
    let yaml = r#"version: 1
assets:
  brand-logo:
    kind: vector
    role: illustration
    usage_hint: "Primary brand logo for splash and about screens"
    source: assets/brand-logo.svg
"#;
    let (_tmp, assets_path) = write_assets_project(yaml, &["assets/brand-logo.svg"]);
    let args = Args {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "usage_hint alone should validate cleanly: {envelope}"
    );
}
