//! `specify vectis validate <mode> [path]` -- schema and cross-artifact
//! validation surface (RFC-11 §H, §I).
//!
//! Phase 1.6 ships the `tokens` mode against the embedded
//! `schemas/vectis/tokens.schema.json` (Appendix A; vendored from the
//! `specify` repo at `crates/vectis/embedded/tokens.schema.json`). The
//! remaining four modes (`layout`, `composition`, `assets`, `all`)
//! still return [`CommandOutcome::Stub`] and will be filled in by
//! Phases 1.7-1.10:
//!
//! - **Phase 1.7** -- `assets` mode validates against
//!   `schemas/vectis/assets.schema.json` (Appendix B) plus referenced
//!   file existence and per-platform density coverage (§E).
//! - **Phase 1.8** -- `layout` mode validates as the unwired subset of
//!   `composition.schema.json`, including the §G structural-identity
//!   rule for any `component:` directives present.
//! - **Phase 1.9** -- `composition` mode adds cross-artifact resolution
//!   and auto-invokes `tokens` / `assets` when sibling files exist.
//! - **Phase 1.10** -- `all` runs the four modes in turn, plus the
//!   `artifacts:`-block default-path resolution every mode shares.
//!
//! ## Per-mode envelope
//!
//! Phase 1.5 fixed the JSON shape every mode populates so the
//! dispatcher's `render_validate_text` and exit-code helper can stay
//! mode-agnostic:
//!
//! ```json
//! {
//!   "mode": "tokens",
//!   "path": "design-system/tokens.yaml",
//!   "errors":   [{ "path": "/colors/primary/light", "message": "..." }],
//!   "warnings": [{ "path": "/typography/...",       "message": "..." }]
//! }
//! ```
//!
//! Errors / warnings entries carry a JSON Pointer-shaped `path` (the
//! same `instance_path` jsonschema reports) so operators can find the
//! offending sub-document quickly. The dispatcher (in
//! `src/commands/vectis.rs::run_vectis`) translates `errors.is_empty()
//! -> exit 0` and `errors.non_empty -> exit 1` per RFC-11 §H ("non-zero
//! on errors, zero with a printed warning report on warnings, zero
//! silently on a clean run").

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::{Value, json};

use crate::error::VectisError;
use crate::{CommandOutcome, ValidateArgs, ValidateMode};

/// Embedded `tokens.schema.json` (RFC-11 Appendix A). Vendored from
/// the `specify` repo at `schemas/vectis/tokens.schema.json` (Phase
/// 1.1). Keep the two files in lock-step -- the upstream copy is the
/// source of truth and any edit there must be mirrored here so the
/// CLI validator and the on-disk schema agree.
const TOKENS_SCHEMA_SOURCE: &str = include_str!("../embedded/tokens.schema.json");

/// Default fallback path for `tokens.yaml` when no `[path]` argument is
/// supplied (RFC-11 §H "Inputs"). Phase 1.10 layers `artifacts:`-block
/// resolution on top of this; until then the canonical fallback is
/// the project-relative path documented in the RFC.
const DEFAULT_TOKENS_PATH: &str = "design-system/tokens.yaml";

/// Lazily compiled tokens validator. Compiling once per process avoids
/// re-parsing the embedded schema on every invocation; in practice the
/// CLI runs one mode per process today, but Phase 1.10's `validate
/// all` will fan out and exercise every mode in a single dispatch.
static TOKENS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Dispatch a `vectis validate` invocation to the per-mode handler.
///
/// Phase 1.6 implements `tokens`; the other four modes still return
/// [`CommandOutcome::Stub`] with a `command` string of the form
/// `"validate <mode>"` so the dispatcher in `src/commands/vectis.rs`
/// emits the v2 `not-implemented` envelope unchanged.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved
/// `tokens.yaml` is unreadable (missing file, permission denied) and
/// [`VectisError::Internal`] if the embedded `tokens.schema.json`
/// fails to compile (a build-time invariant violation -- both files
/// ship with the binary). YAML parse failures and schema validation
/// failures are *not* errors at this layer; they are folded into the
/// `errors` array of the per-mode envelope so the operator sees the
/// full report alongside any other findings.
pub fn run(args: &ValidateArgs) -> Result<CommandOutcome, VectisError> {
    match args.mode {
        ValidateMode::Tokens => validate_tokens(args.path.as_deref()),
        mode => Ok(CommandOutcome::Stub {
            command: stub_command(mode),
        }),
    }
}

/// Stub command identifier for not-yet-implemented modes. The string
/// MUST match the kebab-case spelling in [`ValidateMode::as_str`] so
/// the v2 `not-implemented` envelope's `command` field stays
/// consistent across modes.
const fn stub_command(mode: ValidateMode) -> &'static str {
    match mode {
        ValidateMode::Layout => "validate layout",
        ValidateMode::Composition => "validate composition",
        ValidateMode::Tokens => "validate tokens",
        ValidateMode::Assets => "validate assets",
        ValidateMode::All => "validate all",
    }
}

/// Validate `tokens.yaml` against the embedded Appendix A schema.
///
/// Resolution order for the file path:
/// 1. The explicit `[path]` positional, when supplied.
/// 2. The canonical fallback `design-system/tokens.yaml` (relative to
///    the current working directory).
///
/// Phase 1.10 adds an `artifacts:`-block lookup between (1) and (2);
/// until then the canonical fallback is the only default the CLI
/// honours.
fn validate_tokens(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target =
        path.map_or_else(|| PathBuf::from(DEFAULT_TOKENS_PATH), std::path::Path::to_path_buf);

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("tokens.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = tokens_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(json!({
                    "path": err.instance_path().to_string(),
                    "message": err.to_string(),
                }));
            }
        }
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
        }
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::Tokens.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        // Tokens validation has no warning class today (Appendix A is
        // pure structural shape). Phase 1.7's `assets` mode and Phase
        // 1.8's `layout` mode introduce warnings (missing optional
        // densities, candidate-component flags, etc.); the array stays
        // here so the envelope shape is uniform across modes.
        "warnings": Vec::<Value>::new(),
    })))
}

/// Compile the embedded tokens schema once and re-use the
/// [`Validator`] for every invocation in this process.
///
/// Returns [`VectisError::Internal`] if the embedded JSON is
/// unparseable or the schema fails to compile. Both are build-time
/// invariants -- the embedded copy is vendored from the upstream
/// `tokens.schema.json` which `make checks` validates -- so any
/// runtime hit here implies the wrong file was pulled into the binary
/// at compile time.
fn tokens_validator() -> Result<&'static Validator, VectisError> {
    let entry = TOKENS_VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(TOKENS_SCHEMA_SOURCE)
            .map_err(|err| format!("embedded tokens.schema.json is not JSON: {err}"))?;
        jsonschema::validator_for(&schema)
            .map_err(|err| format!("embedded tokens.schema.json failed to compile: {err}"))
    });
    match entry {
        Ok(validator) => Ok(validator),
        Err(message) => Err(VectisError::Internal {
            message: message.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Appendix D verbatim. Pinned here as a unit test so the embedded
    /// schema stays in lock-step with the RFC's worked example -- if a
    /// future drift breaks Appendix D, this is where the breakage
    /// surfaces first.
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

typography:
  caption:
    size: 12
    weight: regular
    lineHeight: 16
  body:
    size: 16
    weight: regular
    lineHeight: 24
  title:
    size: 22
    weight: semibold
    lineHeight: 28
  display:
    size: 32
    weight: bold
    lineHeight: 40
    letterSpacing: -0.5

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
    radius: 8

opacity:
  disabled: 0.38
  scrim: 0.4
"##;

    fn write_tokens(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("tempfile");
        file.write_all(content.as_bytes()).expect("write tokens.yaml");
        file
    }

    fn extract_envelope(outcome: CommandOutcome) -> Value {
        match outcome {
            CommandOutcome::Success(value) => value,
            CommandOutcome::Stub { command } => {
                panic!("expected Success envelope from `tokens` mode, got Stub({command})")
            }
        }
    }

    fn errors_array(envelope: &Value) -> &[Value] {
        envelope.get("errors").and_then(Value::as_array).expect("errors array").as_slice()
    }

    fn warnings_array(envelope: &Value) -> &[Value] {
        envelope.get("warnings").and_then(Value::as_array).expect("warnings array").as_slice()
    }

    #[test]
    fn embedded_tokens_schema_compiles() {
        tokens_validator().expect("embedded tokens.schema.json must compile");
    }

    #[test]
    fn appendix_d_validates_cleanly() {
        let file = write_tokens(APPENDIX_D_TOKENS_YAML);
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert_eq!(envelope["mode"], "tokens");
        assert!(errors_array(&envelope).is_empty(), "Appendix D unexpectedly errored: {envelope}");
        assert!(warnings_array(&envelope).is_empty(), "no warnings expected: {envelope}");
    }

    #[test]
    fn minimal_version_only_document_is_valid() {
        let file = write_tokens("version: 1\n");
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert!(errors_array(&envelope).is_empty(), "{envelope}");
    }

    #[test]
    fn broken_hex_reports_a_pathful_error() {
        let yaml = "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n";
        let file = write_tokens(yaml);
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        assert!(!errors.is_empty(), "expected at least one error for invalid hex: {envelope}");
        let any_path_hits_primary_light = errors.iter().any(|e| {
            e.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| p.contains("/colors/primary/light"))
        });
        assert!(
            any_path_hits_primary_light,
            "expected an error pointing at /colors/primary/light, got: {errors:?}"
        );
    }

    #[test]
    fn unknown_provenance_kind_is_rejected() {
        let yaml = "version: 1\nprovenance:\n  sources:\n    - kind: screenshots\n";
        let file = write_tokens(yaml);
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        // tokens.schema.json's provenance enum is the §F six values
        // (`manual, figma-variables, style-dictionary, tokens-studio,
        // dtcg, legacy`); `screenshots` is the composition-schema
        // value (Phase 1.3) and MUST NOT leak into tokens.
        let errors = errors_array(&envelope);
        assert!(
            !errors.is_empty(),
            "expected `screenshots` to be rejected by tokens schema: {envelope}"
        );
    }

    #[test]
    fn invalid_yaml_surfaces_as_a_single_error_entry() {
        let file = write_tokens(": : not valid yaml :::\n");
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
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
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(PathBuf::from("/definitely/not/here/tokens.yaml")),
        };
        match run(&args) {
            Err(VectisError::InvalidProject { message }) => {
                assert!(
                    message.contains("tokens.yaml not readable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected InvalidProject for missing file, got {other:?}"),
        }
    }

    /// Stub modes (every mode except `tokens` after Phase 1.6) MUST
    /// continue to return [`CommandOutcome::Stub`] until the
    /// corresponding phase lands. This pins the regression so
    /// accidentally flipping a mode to `Success` shows up in CI.
    #[test]
    fn non_tokens_modes_still_return_stub() {
        for (mode, expected) in [
            (ValidateMode::Layout, "validate layout"),
            (ValidateMode::Composition, "validate composition"),
            (ValidateMode::Assets, "validate assets"),
            (ValidateMode::All, "validate all"),
        ] {
            let args = ValidateArgs { mode, path: None };
            let outcome = run(&args).expect("stub never errors");
            match outcome {
                CommandOutcome::Stub { command } => assert_eq!(command, expected),
                CommandOutcome::Success(value) => {
                    panic!("expected Stub for {mode:?}, got Success({value})")
                }
            }
        }
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
}
