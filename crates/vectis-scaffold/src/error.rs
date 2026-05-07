//! Error types for render-only Vectis scaffold planning and writes.

use std::io;

use thiserror::Error;

/// Terminal failure modes for `vectis-scaffold`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ScaffoldError {
    /// Filesystem I/O failure.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// The invocation or target project state is invalid.
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

impl ScaffoldError {
    /// Process exit code for this error.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        1
    }

    /// Kebab-case identifier for the structured error payload.
    #[must_use]
    pub const fn variant_str(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::InvalidProject { .. } => "invalid-project",
            Self::Internal { .. } => "internal",
        }
    }

    /// Render this error as a JSON payload.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Io(err) => serde_json::json!({
                "error": self.variant_str(),
                "message": err.to_string(),
            }),
            Self::InvalidProject { message } | Self::Internal { message } => serde_json::json!({
                "error": self.variant_str(),
                "message": message,
            }),
        }
    }
}
