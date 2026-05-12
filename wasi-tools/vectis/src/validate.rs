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

/// Process exit code for clean validation.
pub const EXIT_CLEAN: u8 = 0;

/// Process exit code for successful validation runs that found errors.
pub const EXIT_FINDINGS: u8 = 1;

/// Process exit code for invocation, I/O, and runtime failures.
pub const EXIT_FAILURE: u8 = 2;

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

/// Outcome returned by the validation engine.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommandOutcome {
    /// Handler completed normally with a JSON payload.
    Success(Value),
}

/// Error types used by deterministic validation.
pub mod error {
    use serde_json::Value;
    use thiserror::Error;

    /// Terminal validation failures that are not validation findings.
    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum VectisError {
        /// The project structure or requested input is invalid or unreadable.
        #[error("invalid project: {message}")]
        InvalidProject {
            /// Diagnostic describing what is wrong.
            message: String,
        },

        /// An internal invariant was violated.
        #[error("internal error: {message}")]
        Internal {
            /// Diagnostic describing what went wrong.
            message: String,
        },
    }

    impl VectisError {
        /// Process exit code for this error.
        #[must_use]
        pub const fn exit_code(&self) -> u8 {
            crate::validate::EXIT_FAILURE
        }

        /// Kebab-case identifier used in the structured JSON payload.
        #[must_use]
        pub const fn variant_str(&self) -> &'static str {
            match self {
                Self::InvalidProject { .. } => "invalid-project",
                Self::Internal { .. } => "internal",
            }
        }

        /// Render the error as the structured JSON shape.
        #[must_use]
        pub fn to_json(&self) -> Value {
            match self {
                Self::InvalidProject { message } | Self::Internal { message } => {
                    serde_json::json!({
                        "error": self.variant_str(),
                        "message": message,
                    })
                }
            }
        }
    }
}

pub use error::VectisError;

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
pub fn render_json(outcome: Result<CommandOutcome, VectisError>) -> (String, u8) {
    match outcome {
        Ok(CommandOutcome::Success(value)) => {
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

    if has_errors(value) { EXIT_FINDINGS } else { EXIT_CLEAN }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn render_success_payload_carries_mode_and_exits_clean() {
        let (json, code) = render_json(Ok(CommandOutcome::Success(json!({
            "mode": "tokens",
            "path": "tokens.yaml",
            "errors": [],
            "warnings": [],
        }))));

        assert_eq!(code, EXIT_CLEAN);
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

        assert_eq!(validate_exit_code(&payload), EXIT_FINDINGS);
    }

    #[test]
    fn runtime_errors_exit_two_with_typed_error_payload() {
        let (json, code) = render_json(Err(VectisError::InvalidProject {
            message: "tokens.yaml not readable".into(),
        }));

        assert_eq!(code, EXIT_FAILURE);
        let value: Value = serde_json::from_str(&json).expect("json body");
        assert_eq!(value["error"], "invalid-project");
        assert_eq!(value["exit-code"], EXIT_FAILURE);
    }
}
