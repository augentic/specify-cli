//! Composition-mode integration tests. Exercises the lifecycle
//! artifact validation (schema + structural-identity + auto-invoke +
//! cross-artifact resolution) end-to-end.

mod engine_support;

use std::path::{Path, PathBuf};

use engine_support::{errors_array, extract_envelope, warnings_array};
use serde_json::Value;
use tempfile::TempDir;
use specify_vectis::validate::error::VectisError;
use specify_vectis::validate::{ValidateArgs as Args, ValidateMode, run};

/// Materialise a composition document plus optional sibling
/// `tokens.yaml` and `assets.yaml` on disk under a fresh tempdir,
/// returning the tempdir and the composition path. The two helpers
/// default to placing the inputs in the same directory (the
/// change-local shape that the sibling-discovery walker picks up
/// first).
fn write_composition_project(
    composition: &str, tokens: Option<&str>, assets: Option<&str>,
) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Mark the tree as a Specify project so the discovery walk-up can
    // stop at the right anchor even when we're testing the
    // design-system fallback shape elsewhere.
    std::fs::create_dir_all(tmp.path().join(".specify")).expect("mkdir .specify");
    let comp_path = tmp.path().join("composition.yaml");
    std::fs::write(&comp_path, composition).expect("write composition.yaml");
    if let Some(yaml) = tokens {
        std::fs::write(tmp.path().join("tokens.yaml"), yaml).expect("write tokens.yaml");
    }
    if let Some(yaml) = assets {
        std::fs::write(tmp.path().join("assets.yaml"), yaml).expect("write assets.yaml");
    }
    (tmp, comp_path)
}

fn run_composition(comp_path: &Path) -> Value {
    let args = Args {
        mode: ValidateMode::Composition,
        path: Some(comp_path.to_path_buf()),
    };
    extract_envelope(run(&args).expect("run succeeds"))
}

/// Acceptance baseline: a minimal valid composition with no sibling
/// tokens / assets validates cleanly. The envelope SHOULD NOT carry a
/// `results` array when no sibling files were found (the array is
/// only emitted when auto-invoke folded something in).
#[test]
fn composition_clean_run_validates_silently_without_siblings() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - text:
              content: hello
";
    let (_tmp, comp_path) = write_composition_project(yaml, None, None);
    let envelope = run_composition(&comp_path);
    assert_eq!(envelope["mode"], "composition");
    assert!(errors_array(&envelope).is_empty(), "errors unexpected: {envelope}");
    assert!(warnings_array(&envelope).is_empty(), "warnings unexpected: {envelope}");
    assert!(
        envelope.get("results").is_none(),
        "results array should be absent without auto-invoke: {envelope}"
    );
}

/// Composition mode (unlike layout mode) MUST allow define-owned
/// wiring keys (`bind`, `event`, `error`, overlay `trigger`,
/// `*-when`) and `delta:` shape. This pins the contract distinction
/// that justifies two runtime layers over the same schema.
#[test]
fn composition_permits_wired_keys_layout_rejects() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    maps_to: SomeRoute
    body:
      list:
        each: tasks
        item:
          - checkbox:
              bind: tasks.completed
              event: ToggleTask
              strikethrough-when: tasks.completed
    overlays:
      sheet:
        kind: sheet
        trigger: OpenSheet
        content:
          - text:
              content: hi
";
    let (_tmp, comp_path) = write_composition_project(yaml, None, None);
    let envelope = run_composition(&comp_path);
    assert!(
        errors_array(&envelope).is_empty(),
        "wired keys MUST validate cleanly in composition mode: {envelope}"
    );
}

/// `delta:` documents are valid in composition mode (the change-local
/// lifecycle shape). The schema's `oneOf` accepts either `screens` or
/// `delta`.
#[test]
fn composition_accepts_delta_documents() {
    let yaml = r"version: 1
delta:
  added:
    new-screen:
      name: New
      body:
        list:
          each: things
          item:
            - text:
                content: hello
  modified:
    other:
      name: Other
      body:
        - text:
            content: hi
";
    let (_tmp, comp_path) = write_composition_project(yaml, None, None);
    let envelope = run_composition(&comp_path);
    assert!(errors_array(&envelope).is_empty(), "delta MUST validate cleanly: {envelope}");
}

/// A token reference (`color: nonexistent`) that is absent from the
/// sibling `tokens.yaml` produces a composition-mode error pointing
/// at the offending node. This is the cross-artifact resolution layer
/// the auto-invoke does NOT cover (the auto-invoke catches
/// "tokens.yaml is itself broken" -- this catches "composition
/// references something tokens.yaml does not declare").
#[test]
fn composition_unresolved_color_token_is_an_error() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
          color: nonexistent
";
    let tokens = r##"version: 1
colors:
  primary:
    light: "#0066CC"
    dark: "#3399FF"
"##;
    let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown colors token `nonexistent`")
            && e["path"].as_str().unwrap_or("").ends_with("/text/color")),
        "expected an unresolved-color error: {errors:?}"
    );
}

/// String-valued `gap: <name>` references resolve against
/// `spacing.<name>`. A typo (`gap: mid` instead of `md`) MUST surface
/// as an error.
#[test]
fn composition_unresolved_spacing_token_is_an_error() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          gap: mid
          items:
            - text:
                content: hi
";
    let tokens = r"version: 1
spacing:
  xs: 4
  sm: 8
  md: 16
  lg: 24
";
    let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors
            .iter()
            .any(|e| e["message"].as_str().unwrap_or("").contains("unknown spacing token `mid`")),
        "expected an unresolved-spacing error: {errors:?}"
    );
}

/// Numeric `gap: 16` MUST NOT surface a token-resolution error -- it
/// is a literal pixel value. This pins the string-or-number split at
/// the resolver layer.
#[test]
fn composition_numeric_spacing_is_not_a_token_ref() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          gap: 16
          padding: 8
          items:
            - text:
                content: hi
";
    let tokens = r"version: 1
spacing:
  xs: 4
";
    let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
    let envelope = run_composition(&comp_path);
    assert!(
        errors_array(&envelope).is_empty(),
        "numeric spacing values MUST NOT trip the resolver: {envelope}"
    );
}

/// `padding` may be a paddingSpec object with per-side string values
/// (`top: md`, etc.). Each side resolves against `spacing.<name>`
/// independently.
#[test]
fn composition_padding_object_resolves_per_side() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          padding:
            top: md
            bottom: lg
            left: nope
          items:
            - text:
                content: hi
";
    let tokens = r"version: 1
spacing:
  md: 16
  lg: 24
";
    let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["path"].as_str().unwrap_or("").ends_with("/padding/left")
            && e["message"].as_str().unwrap_or("").contains("unknown spacing token `nope`")),
        "expected an unresolved-padding-side error: {errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e["path"].as_str().unwrap_or("").ends_with("/padding/top")
            || e["path"].as_str().unwrap_or("").ends_with("/padding/bottom")),
        "valid padding sides must not surface: {errors:?}"
    );
}

/// Elevation tokens resolve against `elevation.<name>` and
/// `corner_radius` tokens against `cornerRadius.<name>`. A typo in
/// either category surfaces as an error.
#[test]
fn composition_unresolved_elevation_and_corner_radius_are_errors() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          elevation: floating
          corner_radius: huge
          items:
            - text:
                content: hi
";
    let tokens = r"version: 1
elevation:
  card: 2
cornerRadius:
  md: 8
";
    let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown elevation token `floating`")),
        "expected an unresolved-elevation error: {errors:?}"
    );
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown cornerRadius token `huge`")),
        "expected an unresolved-cornerRadius error: {errors:?}"
    );
}

/// Asset references (`image.name`, `icon.name`, `icon-button.icon`,
/// `fab.icon`) that point at unknown ids in the sibling `assets.yaml`
/// produce composition-mode errors via the shared
/// `collect_asset_references` walker.
#[test]
fn composition_unresolved_asset_id_is_an_error() {
    let composition = r"version: 1
screens:
  s:
    name: S
    header:
      title: T
      trailing:
        - icon-button:
            icon: mystery
            label: Mystery
    body:
      - image:
          name: empty-tasks-hero
";
    let assets = r"version: 1
assets:
  empty-tasks-hero:
    kind: symbol
    role: icon
    symbols:
      ios: foo
      android: bar
";
    let (_tmp, comp_path) = write_composition_project(composition, None, Some(assets));
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown asset id `mystery`")
            && e["path"].as_str().unwrap_or("").ends_with("/icon-button/icon")),
        "expected an unresolved-asset error: {errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e["message"].as_str().unwrap_or("").contains("`empty-tasks-hero`")),
        "valid asset id MUST resolve cleanly: {errors:?}"
    );
}

/// Auto-invoke: when a sibling `tokens.yaml` exists, the composition
/// envelope's `results` array MUST contain a `tokens` report. A
/// broken hex inside that tokens.yaml surfaces as an error inside
/// `results[].report.errors`, which the dispatcher's
/// `validate_exit_code` recurses through.
#[test]
fn composition_auto_invokes_tokens_and_folds_into_results() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
";
    let broken_tokens = r##"version: 1
colors:
  primary:
    light: "#xyz"
    dark: "#000000"
"##;
    let (_tmp, comp_path) = write_composition_project(composition, Some(broken_tokens), None);
    let envelope = run_composition(&comp_path);
    let results = envelope["results"].as_array().expect("results array present");
    assert_eq!(results.len(), 1, "expected exactly one folded sub-report: {envelope}");
    assert_eq!(results[0]["mode"], "tokens");
    let tokens_errors =
        results[0]["report"]["errors"].as_array().expect("nested tokens.errors is an array");
    assert!(
        !tokens_errors.is_empty(),
        "expected the broken hex to surface in the folded tokens report: {envelope}"
    );
}

/// Auto-invoke: when both sibling tokens and assets exist, both
/// reports surface and the order in `results` is `tokens` before
/// `assets` (matches the order `validate all` will ship).
#[test]
fn composition_auto_invokes_tokens_and_assets_in_order() {
    let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
";
    let tokens = r"version: 1
spacing:
  md: 16
";
    let assets = r"version: 1
assets: {}
";
    let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), Some(assets));
    let envelope = run_composition(&comp_path);
    let results = envelope["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "expected two folded sub-reports: {envelope}");
    assert_eq!(results[0]["mode"], "tokens");
    assert_eq!(results[1]["mode"], "assets");
}

/// Structural-identity reuses the layout engine. Two `component: card`
/// instances with materially different skeletons in the `screens`
/// shape MUST produce a composition-mode error.
#[test]
fn composition_structural_identity_violation_in_screens() {
    let composition = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - text:
                content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - icon:
                name: chevron-right
            - text:
                content: body
";
    let (_tmp, comp_path) = write_composition_project(composition, None, None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("component slug `card` has a different skeleton")),
        "expected structural-identity error in screens shape: {errors:?}"
    );
}

/// Structural-identity walks the `delta` sub-tree too: a slug added
/// in `delta.added` must agree with the same slug modified in
/// `delta.modified`.
#[test]
fn composition_structural_identity_violation_in_delta() {
    let composition = r"version: 1
delta:
  added:
    one:
      name: One
      body:
        - group:
            component: card
            direction: column
            items:
              - text:
                  content: heading
              - text:
                  content: body
  modified:
    two:
      name: Two
      body:
        - group:
            component: card
            direction: column
            items:
              - text:
                  content: heading
              - icon:
                  name: chevron-right
              - text:
                  content: body
";
    let (_tmp, comp_path) = write_composition_project(composition, None, None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("component slug `card` has a different skeleton")),
        "expected structural-identity error in delta shape: {errors:?}"
    );
}

/// The design-system-shape sibling fallback: when the composition
/// lives at `<root>/.specify/specs/composition.yaml` (the canonical
/// baseline location), the discovery walk picks up
/// `<root>/design-system/tokens.yaml` and
/// `<root>/design-system/assets.yaml`.
#[test]
fn composition_design_system_fallback_picks_up_siblings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let specs_dir = tmp.path().join(".specify/specs");
    let design_dir = tmp.path().join("design-system");
    std::fs::create_dir_all(&specs_dir).expect("mkdir .specify/specs");
    std::fs::create_dir_all(&design_dir).expect("mkdir design-system");
    let comp_path = specs_dir.join("composition.yaml");
    std::fs::write(
        &comp_path,
        r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
          color: surface
",
    )
    .expect("write composition.yaml");
    std::fs::write(
        design_dir.join("tokens.yaml"),
        r##"version: 1
colors:
  surface:
    light: "#FFFFFF"
    dark: "#000000"
"##,
    )
    .expect("write design-system/tokens.yaml");
    std::fs::write(design_dir.join("assets.yaml"), "version: 1\nassets: {}\n")
        .expect("write design-system/assets.yaml");

    let envelope = run_composition(&comp_path);
    assert!(
        errors_array(&envelope).is_empty(),
        "design-system fallback path MUST resolve cleanly: {envelope}"
    );
    let results = envelope["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "expected tokens + assets fallback fold: {envelope}");
}

/// Reserved component slugs (header / body / footer / fab) are
/// rejected by the F.2 patch's `not.enum` -- composition mode
/// surfaces this as a schema error just like layout mode does.
#[test]
fn composition_reserved_component_slug_is_rejected() {
    let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          component: header
          direction: column
          items:
            - text:
                content: hi
";
    let (_tmp, comp_path) = write_composition_project(yaml, None, None);
    let envelope = run_composition(&comp_path);
    assert!(
        !errors_array(&envelope).is_empty(),
        "reserved slug `header` MUST be rejected by the F.2 patch: {envelope}"
    );
}

#[test]
fn composition_invalid_yaml_surfaces_as_a_single_error_entry() {
    let (_tmp, comp_path) = write_composition_project(": : not valid yaml :::\n", None, None);
    let envelope = run_composition(&comp_path);
    let errors = errors_array(&envelope);
    assert_eq!(errors.len(), 1, "expected one YAML-parse error: {envelope}");
    assert!(
        errors[0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
        "expected `invalid YAML` prefix, got {:?}",
        errors[0]
    );
}

#[test]
fn composition_missing_file_returns_invalid_project_error() {
    let args = Args {
        mode: ValidateMode::Composition,
        path: Some(PathBuf::from("/definitely/not/here/composition.yaml")),
    };
    match run(&args) {
        Err(VectisError::InvalidProject { message }) => {
            assert!(
                message.contains("composition.yaml not readable"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected InvalidProject for missing file, got {other:?}"),
    }
}
