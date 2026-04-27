//! Unified error types for the vectis CLI.
//!
//! Every subcommand handler returns `Result<serde_json::Value, VectisError>`. On
//! success the handler emits its result JSON to stdout and exits 0. On failure
//! the dispatcher serializes [`VectisError::to_json`] to stdout and exits with
//! the variant's [`VectisError::exit_code`].

use std::io;

use serde::Serialize;
use thiserror::Error;

/// A single missing tool reported by the prerequisite checker.
///
/// Matches the shape documented in RFC-6 § Prerequisite Detection.
#[derive(Debug, Clone, Serialize)]
pub struct MissingTool {
    /// Stable identifier reported in the JSON payload (e.g. `"xcodegen"`).
    pub tool: String,
    /// Assembly this tool belongs to (`"core"`, `"ios"`, or `"android"`).
    pub assembly: String,
    /// Human-readable command the user can run to verify the tool.
    pub check: String,
    /// Install hint shown to the user.
    pub install: String,
}

/// All terminal failure modes for the CLI.
///
/// Subcommand handlers convert their internal errors into one of these
/// variants. The dispatcher turns the variant into the RFC's structured JSON
/// error shape via [`VectisError::to_json`].
///
/// Every variant is actively constructed today: chunks 1/2 build
/// `MissingPrerequisites`, `Io`, and `InvalidProject`; chunk 4 constructs
/// `Internal` (from `versions::load_embedded` if the embedded defaults
/// are ever malformed); chunks 7/8/9 construct `Verify` from the
/// per-assembly build pipelines.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VectisError {
    /// One or more workstation tools are missing.
    #[error("missing prerequisites: {message}")]
    MissingPrerequisites {
        /// Tools that failed their check.
        missing: Vec<MissingTool>,
        /// Human-readable summary.
        message: String,
    },

    /// Filesystem I/O failure.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// The project structure or configuration is invalid.
    #[error("invalid project: {message}")]
    InvalidProject {
        /// Diagnostic describing what is wrong.
        message: String,
    },

    /// A build or verify step failed.
    #[error("verify failed: {message}")]
    Verify {
        /// Diagnostic describing the failure.
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
    ///
    /// Missing prerequisites is `2` so callers can distinguish "your
    /// workstation is incomplete" from generic failure (`1`).
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::MissingPrerequisites { .. } => 2,
            _ => 1,
        }
    }

    /// Kebab-case identifier for the variant, used as the `error` value
    /// in the structured JSON shape and by the dispatcher when
    /// synthesising the `exit-code`/`message` envelope.
    #[must_use]
    pub const fn variant_str(&self) -> &'static str {
        match self {
            Self::MissingPrerequisites { .. } => "missing-prerequisites",
            Self::Io(_) => "io",
            Self::InvalidProject { .. } => "invalid-project",
            Self::Verify { .. } => "verify",
            Self::Internal { .. } => "internal",
        }
    }

    /// Render the error as the structured JSON shape defined in RFC-6.
    ///
    /// Keys and the `error` variant are kebab-case to match the v2 JSON
    /// contract enforced by the `specify` binary; the dispatcher's
    /// `emit_json` helper auto-injects `schema-version: 2` on top.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::MissingPrerequisites { missing, message } => serde_json::json!({
                "error": self.variant_str(),
                "missing": missing,
                "message": message,
            }),
            Self::Io(err) => serde_json::json!({
                "error": self.variant_str(),
                "message": err.to_string(),
            }),
            Self::InvalidProject { message }
            | Self::Verify { message }
            | Self::Internal { message } => serde_json::json!({
                "error": self.variant_str(),
                "message": message,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_prerequisites_json_shape() {
        let err = VectisError::MissingPrerequisites {
            missing: vec![MissingTool {
                tool: "xcodegen".into(),
                assembly: "ios".into(),
                check: "xcodegen --version".into(),
                install: "brew install xcodegen".into(),
            }],
            message: "Install the missing tools above and re-run the command.".into(),
        };
        let v = err.to_json();
        assert_eq!(v["error"], "missing-prerequisites");
        assert_eq!(v["missing"][0]["tool"], "xcodegen");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn invalid_project_json_shape() {
        let err = VectisError::InvalidProject {
            message: "version file not found: /nonexistent.toml".into(),
        };
        let v = err.to_json();
        assert_eq!(v["error"], "invalid-project");
        assert_eq!(v["message"], "version file not found: /nonexistent.toml");
        assert_eq!(err.exit_code(), 1);
    }
}
