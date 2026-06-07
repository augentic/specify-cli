//! Unified terminal-error type shared by every `vectis` subcommand.
//!
//! Every subcommand reports failures through this one type. The wire
//! payload is `{"error": "...", "message": "..."}` plus an injected
//! `"exit-code"`.

use std::io;

use serde_json::Value;
use thiserror::Error;

/// Process exit code for all terminal `vectis` failures.
///
/// `vectis` standardises on the host CLI's typed-error slot: `0` for
/// clean success, `1` for a successful run that surfaced findings, and
/// `2` for invocation / I/O / runtime failures. Every subcommand
/// reports failures with `2`, matching the host contract and the
/// `validate` exit shape.
pub const EXIT_FAILURE: u8 = 2;

/// Terminal failure modes for any `vectis` subcommand.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VectisError {
    /// Filesystem I/O failure (scaffold writes and version-file reads).
    #[error("io error: {0}")]
    Io(#[from] io::Error),

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
        EXIT_FAILURE
    }

    /// Kebab-case identifier used in the structured JSON payload.
    #[must_use]
    pub const fn variant_str(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::InvalidProject { .. } => "invalid-project",
            Self::Internal { .. } => "internal",
        }
    }

    /// Render the error as the structured JSON shape.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Io(err) => serde_json::json!({
                "error": self.variant_str(),
                "message": err.to_string(),
            }),
            Self::InvalidProject { message } | Self::Internal { message } => {
                serde_json::json!({
                    "error": self.variant_str(),
                    "message": message,
                })
            }
        }
    }
}
