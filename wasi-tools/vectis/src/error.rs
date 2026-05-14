//! Unified terminal-error type shared by every `vectis` subcommand.
//!
//! `validate` and `scaffold` previously carried two near-identical
//! enums (`VectisError` and `ScaffoldError`) that differed only in
//! whether they reported a separate `Io` variant and in the integer
//! exit code they returned. The wire payload — `{"error": "...",
//! "message": "..."}` plus an injected `"exit-code"` — was already
//! byte-identical, so the two types are collapsed here.

use std::io;

use serde_json::Value;
use thiserror::Error;

/// Process exit code for all terminal `vectis` failures.
///
/// `vectis` standardises on the host CLI's typed-error slot: `0` for
/// clean success, `1` for a successful run that surfaced findings, and
/// `2` for invocation / I/O / runtime failures. Scaffolding previously
/// returned `1` for failures; collapsing both subcommands onto `2`
/// matches the host contract and the `validate` exit shape.
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
