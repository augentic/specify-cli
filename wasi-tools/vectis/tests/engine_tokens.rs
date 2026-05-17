//! Tokens-mode integration tests. Exercises the embedded
//! `tokens.schema.json` plus the `specify_vectis::validate::run` dispatcher.

mod engine_support;

use std::path::PathBuf;

use engine_support::{errors_array, warnings_array, write_named};
use serde_json::Value;
use specify_vectis::validate::__test_internals::{assets_validator, tokens_validator};
use specify_vectis::validate::error::VectisError;
use specify_vectis::validate::{ValidateArgs as Args, ValidateMode, run};

/// Appendix D verbatim. Pinned here as an integration test so the
/// embedded schema stays in lock-step with the worked example -- if a
/// future drift breaks Appendix D, this is where the breakage surfaces
/// first.
//
// Uses the `r##"..."##` raw-string delimiter so the embedded
// `"#0066CC"` patterns don't close the literal early.
const APPENDIX_D_TOKENS_YAML: &str = r##"version: 1

provenance:
  sources:
    - kind: figma-variables
      uri: "https://www.figma.com/file/ABC123/Design-System"
      captured_at: "2026-04-10T09:15:00Z"
    - kind: manual

colors:
  primary:
    light: "#0066CC"
    dark: "#3399FF"
  on-primary:
    light: "#FFFFFF"
    dark: "#001F3F"
  surface:
    light: "#FFFFFF"
    dark: "#121212"
  on-surface:
    light: "#1C1B1F"
    dark: "#E6E1E5"
  on-surface-variant:
    light: "#49454F"
    dark: "#CAC4D0"
  outline:
    light: "#79747E"
    dark: "#938F99"
  error:
    light: "#B3261E"
    dark: "#F2B8B5"

typefaces:
  default:
    family: "Inter"
    fallback: "system-ui, sans-serif"
    source: google-fonts
  mono:
    family: "Roboto Mono"
    source: google-fonts

typography:
  caption:
    typeface: default
    size: 12
    weight: regular
    lineHeight: 16
  body:
    size: 16
    weight: regular
    lineHeight: 24
  title:
    typeface: default
    size: 22
    weight: semibold
    lineHeight: 28
  display:
    size: 32
    weight: bold
    lineHeight: 40
    letterSpacing: -0.5
  code-inline:
    typeface: mono
    size: 14
    weight: regular
    lineHeight: 20

spacing:
  xs: 4
  sm: 8
  md: 16
  lg: 24
  xl: 32

cornerRadius:
  sm: 4
  md: 8
  lg: 16

elevation:
  card: 2
  modal: 8

border:
  subtle:
    width: 1
    color: outline
  emphasis:
    width: 2
    color: primary
    radius: md

opacity:
  disabled: 0.38
  scrim: 0.4
"##;

#[test]
fn embedded_tokens_schema_compiles() {
    tokens_validator().expect("embedded tokens.schema.json must compile");
}

#[test]
fn embedded_assets_schema_compiles() {
    assets_validator().expect("embedded assets.schema.json must compile");
}

#[test]
fn appendix_d_validates_cleanly() {
    let file = write_named(APPENDIX_D_TOKENS_YAML);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    assert_eq!(envelope["mode"], "tokens");
    assert!(errors_array(&envelope).is_empty(), "Appendix D unexpectedly errored: {envelope}");
    assert!(warnings_array(&envelope).is_empty(), "no warnings expected: {envelope}");
}

#[test]
fn minimal_version_only_document_is_valid() {
    let file = write_named("version: 1\n");
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(errors_array(&envelope).is_empty(), "{envelope}");
}

#[test]
fn broken_hex_reports_a_pathful_error() {
    let yaml = "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n";
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(!errors.is_empty(), "expected at least one error for invalid hex: {envelope}");
    let any_path_hits_primary_light = errors.iter().any(|e| {
        e.get("path").and_then(Value::as_str).is_some_and(|p| p.contains("/colors/primary/light"))
    });
    assert!(
        any_path_hits_primary_light,
        "expected an error pointing at /colors/primary/light, got: {errors:?}"
    );
}

#[test]
fn unknown_provenance_kind_is_rejected() {
    let yaml = "version: 1\nprovenance:\n  sources:\n    - kind: screenshots\n";
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    // tokens.schema.json's provenance enum is the six values
    // (`manual, figma-variables, style-dictionary, tokens-studio,
    // dtcg, legacy`); `screenshots` is the composition-schema value
    // and MUST NOT leak into tokens.
    let errors = errors_array(&envelope);
    assert!(
        !errors.is_empty(),
        "expected `screenshots` to be rejected by tokens schema: {envelope}"
    );
}

#[test]
fn invalid_yaml_surfaces_as_a_single_error_entry() {
    let file = write_named(": : not valid yaml :::\n");
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert_eq!(errors.len(), 1, "expected one YAML-parse error: {envelope}");
    assert!(
        errors[0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
        "expected `invalid YAML` prefix, got {:?}",
        errors[0]
    );
}

#[test]
fn missing_file_returns_invalid_project_error() {
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(PathBuf::from("/definitely/not/here/tokens.yaml")),
    };
    match run(&args) {
        Err(VectisError::InvalidProject { message }) => {
            assert!(message.contains("tokens.yaml not readable"), "unexpected message: {message}");
        }
        other => panic!("expected InvalidProject for missing file, got {other:?}"),
    }
}

#[test]
fn typefaces_with_typeface_references_validates_cleanly() {
    let yaml = r#"version: 1
typefaces:
  default:
    family: "Inter"
    fallback: "system-ui, sans-serif"
    source: google-fonts
  mono:
    family: "Roboto Mono"
    source: bundled
typography:
  body:
    typeface: default
    size: 16
    weight: regular
    lineHeight: 24
  code:
    typeface: mono
    size: 14
    weight: regular
    lineHeight: 20
"#;
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(errors_array(&envelope).is_empty(), "typefaces + typeface should pass: {envelope}");
}

#[test]
fn typeface_field_accepts_any_valid_token_name() {
    let yaml = "version: 1\ntypography:\n  body:\n    typeface: unknown-face\n    size: 16\n";
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(
        errors_array(&envelope).is_empty(),
        "typeface is a tokenName pattern, not a cross-reference check: {envelope}"
    );
}

#[test]
fn border_radius_as_token_ref_string_passes() {
    let yaml = "version: 1\ncornerRadius:\n  md: 8\nborder:\n  card:\n    width: 1\n    color: outline\n    radius: md\n";
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    assert!(errors_array(&envelope).is_empty(), "string radius should pass: {envelope}");
}

#[test]
fn border_radius_as_number_is_rejected() {
    let yaml = "version: 1\nborder:\n  card:\n    width: 1\n    color: outline\n    radius: 8\n";
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(!errors.is_empty(), "numeric radius should fail: {envelope}");
    let any_hits_radius = errors
        .iter()
        .any(|e| e.get("path").and_then(Value::as_str).is_some_and(|p| p.contains("/radius")));
    assert!(any_hits_radius, "expected error at border radius path: {errors:?}");
}

#[test]
fn typeface_entry_rejects_invalid_source_enum() {
    let yaml = r#"version: 1
typefaces:
  custom:
    family: "MyFont"
    source: cdn
"#;
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(!errors.is_empty(), "invalid source enum should fail: {envelope}");
}

#[test]
fn typeface_entry_requires_family() {
    let yaml = "version: 1\ntypefaces:\n  custom:\n    source: system\n";
    let file = write_named(yaml);
    let args = Args {
        mode: ValidateMode::Tokens,
        path: Some(file.path().to_path_buf()),
    };
    let envelope = run(&args).expect("run succeeds");
    let errors = errors_array(&envelope);
    assert!(!errors.is_empty(), "missing family should fail: {envelope}");
}

#[test]
fn validate_mode_as_str_matches_value_enum_spelling() {
    for (mode, expected) in [
        (ValidateMode::Layout, "layout"),
        (ValidateMode::Composition, "composition"),
        (ValidateMode::Tokens, "tokens"),
        (ValidateMode::Assets, "assets"),
        (ValidateMode::All, "all"),
    ] {
        assert_eq!(mode.as_str(), expected);
    }
}
