//! `vectis validate` subcommand surface.
//!
//! The deterministic validation engine and embedded schemas live in
//! this module so the WASI command surface has a single source of
//! truth. Provenance for every rule lives in the sidecar
//! `DECISIONS.md` at the crate root.

use std::path::PathBuf;

use clap::{Args as ClapArgs, ValueEnum};
use serde_json::Value;

use crate::render_json as render_value;

/// Arguments accepted by `vectis validate`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct ValidateArgs {
    /// Validation mode to run.
    #[arg(value_enum)]
    pub mode: ValidateMode,

    /// Artifact path for single-artifact modes, or project root for `all`.
    pub path: Option<PathBuf>,
}

/// Vectis validation modes preserved for the WASI command surface.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ValidateMode {
    /// Validate a `tokens.yaml` file.
    Tokens,
    /// Validate an `assets.yaml` file.
    Assets,
    /// Validate a `layout.yaml` file.
    Layout,
    /// Validate a `composition.yaml` file.
    Composition,
    /// Validate all Vectis UI artifacts reachable from the given root.
    All,
}

impl ValidateMode {
    /// Return the stable CLI spelling for this mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Assets => "assets",
            Self::Layout => "layout",
            Self::Composition => "composition",
            Self::All => "all",
        }
    }
}

/// Re-export the crate-wide error type at its historical path.
///
/// External tests and the engine modules import
/// `specify_vectis::validate::error::VectisError`; the type itself
/// now lives at the crate root so `scaffold` can share it.
pub mod error {
    pub use crate::VectisError;
}

pub use crate::VectisError;

mod engine;

pub use engine::run;

/// Hidden re-exports for integration tests under `crates/vectis/tests/`.
/// These items are not part of the stable public API; they exist so the
/// per-mode test suites can exercise the internal resolver and
/// validator helpers without duplicating fixtures.
#[doc(hidden)]
pub mod __test_internals {
    pub use crate::validate::engine::{
        assets_validator, composition_validator, discover_artifact, expand_path_template,
        find_project_root, paths_for_key, resolve_default_path_with_root, tokens_validator,
    };
}

/// Render a validation outcome as pretty-printed JSON, without a trailing
/// newline, and return the process exit code that should accompany it.
#[must_use]
pub fn render_json(outcome: Result<Value, VectisError>) -> (String, u8) {
    match outcome {
        Ok(value) => {
            let code = validate_exit_code(&value);
            (render_value(&value), code)
        }
        Err(err) => {
            let exit_code = err.exit_code();
            let Value::Object(mut payload) = err.to_json() else {
                unreachable!("VectisError::to_json always returns an object")
            };
            payload.entry("exit-code".to_string()).or_insert(Value::from(exit_code));
            (render_value(&Value::Object(payload)), exit_code)
        }
    }
}

/// Compute the recursive validation exit code for a success payload.
#[must_use]
pub fn validate_exit_code(value: &Value) -> u8 {
    fn has_errors(node: &Value) -> bool {
        if node.get("errors").and_then(Value::as_array).is_some_and(|arr| !arr.is_empty()) {
            return true;
        }
        if let Some(results) = node.get("results").and_then(Value::as_array) {
            return results
                .iter()
                .any(|entry| entry.get("report").is_some_and(has_errors) || has_errors(entry));
        }
        false
    }

    u8::from(has_errors(value))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn render_success_payload_carries_mode_and_exits_clean() {
        let (json, code) = render_json(Ok(json!({
            "mode": "tokens",
            "path": "tokens.yaml",
            "errors": [],
            "warnings": [],
        })));

        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&json).expect("json body");
        assert_eq!(value["mode"], "tokens");
    }

    #[test]
    fn validate_exit_code_recurses_through_results_reports() {
        let payload = json!({
            "mode": "all",
            "results": [{
                "mode": "tokens",
                "report": {
                    "mode": "tokens",
                    "errors": [{ "path": "/colors/bad", "message": "bad color" }],
                    "warnings": [],
                },
            }],
        });

        assert_eq!(validate_exit_code(&payload), 1);
    }

    #[test]
    fn runtime_errors_exit_two_with_typed_error_payload() {
        let (json, code) = render_json(Err(VectisError::InvalidProject {
            message: "tokens.yaml not readable".into(),
        }));

        assert_eq!(code, 2);
        let value: Value = serde_json::from_str(&json).expect("json body");
        assert_eq!(value["error"], "invalid-project");
        assert_eq!(value["exit-code"], 2);
    }
}
