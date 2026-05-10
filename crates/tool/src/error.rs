//! Tool-specific errors and conversions into the shared Specify error surface.

use std::path::PathBuf;

/// Errors produced by declared-tool manifest loading, validation, and cache
/// helpers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolError {
    /// Reading `tools.yaml` failed.
    #[error("tool manifest read failed at {}: {source}", path.display())]
    ManifestRead {
        /// Manifest path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Parsing `tools.yaml` failed.
    #[error("tool manifest parse failed at {}: {source}", path.display())]
    ManifestParse {
        /// Manifest path.
        path: PathBuf,
        /// Underlying YAML error.
        #[source]
        source: Box<serde_saphyr::Error>,
    },
    /// A deterministic cache root could not be selected from the environment.
    #[error("tool cache root error: {0}")]
    CacheRoot(String),
    /// A filesystem operation failed while manipulating tool cache state.
    #[error("tool cache I/O failed while {action} {}: {source}", path.display())]
    CacheIo {
        /// Operation being attempted.
        action: &'static str,
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `meta.yaml` existed but could not be parsed as YAML.
    #[error("invalid tool sidecar YAML at {}: {source}", path.display())]
    SidecarParse {
        /// Path to the sidecar file.
        path: PathBuf,
        /// YAML parse or deserialize error.
        #[source]
        source: Box<serde_saphyr::Error>,
    },
    /// `meta.yaml` parsed, but did not satisfy the sidecar schema.
    #[error("invalid tool sidecar schema at {}: {detail}", path.display())]
    SidecarSchema {
        /// Path to the sidecar file.
        path: PathBuf,
        /// Human-readable validation detail.
        detail: String,
    },
    /// An atomic cache install or replacement step failed.
    #[error("atomic cache move failed from {} to {}: {source}", from.display(), to.display())]
    AtomicMoveFailed {
        /// Source path of the failed move.
        from: PathBuf,
        /// Destination path of the failed move.
        to: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The requested tool name was not present in merged declarations.
    #[error("tool not declared: {name}")]
    ToolNotDeclared {
        /// Missing tool name.
        name: String,
    },
    /// Two declaration sites declared the same tool name.
    #[error("tool name collision: {name}")]
    ToolNameCollision {
        /// Colliding tool name.
        name: String,
    },
    /// A cache path segment would be empty or escape its intended directory.
    #[error("invalid tool cache segment `{value}` for {field}: {reason}")]
    InvalidCacheSegment {
        /// Field being converted to an on-disk path segment.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Rejection reason.
        reason: &'static str,
    },
    /// A tool permission template or expanded permission path is invalid.
    #[error("invalid tool permission `{template}`: {reason}")]
    InvalidPermission {
        /// Rejected manifest permission template.
        template: String,
        /// Rejection reason.
        reason: String,
    },
    /// A requested preopen or filesystem authority is denied.
    #[error("tool permission denied for {}: {reason}", path.display())]
    PermissionDenied {
        /// Denied path.
        path: PathBuf,
        /// Rejection reason.
        reason: String,
    },
    /// Wasmtime failed to compile, link, instantiate, or run a tool component.
    #[error("tool runtime error: {0}")]
    Runtime(String),
    /// A source declaration is not usable by the resolver.
    #[error("tool source `{source_value}` is invalid: {reason}")]
    InvalidSource {
        /// Rejected source string.
        source_value: String,
        /// Rejection reason.
        reason: String,
    },
    /// Reading local source bytes failed.
    #[error("tool source read failed while {action} {}: {source}", path.display())]
    SourceIo {
        /// Operation being attempted.
        action: &'static str,
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Source bytes were empty.
    #[error("tool source `{source_value}` produced empty bytes")]
    EmptySource {
        /// Source string from the live declaration.
        source_value: String,
    },
    /// Source bytes did not match a declared SHA-256 digest.
    #[error("tool source `{source_value}` sha256 mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        /// Source string from the live declaration.
        source_value: String,
        /// Expected lowercase hex SHA-256 digest.
        expected: String,
        /// Actual lowercase hex SHA-256 digest.
        actual: String,
    },
    /// An HTTPS source returned a non-200 status.
    #[error("tool source `{url}` returned HTTP status {status}; expected 200")]
    NetworkStatus {
        /// Requested HTTPS URL.
        url: String,
        /// HTTP status code returned by the server.
        status: u16,
    },
    /// An HTTPS source timed out.
    #[error("tool source `{url}` timed out: {detail}")]
    NetworkTimeout {
        /// Requested HTTPS URL.
        url: String,
        /// Transport detail.
        detail: String,
    },
    /// An HTTPS source URL was malformed.
    #[error("tool source `{url}` is malformed: {detail}")]
    NetworkMalformed {
        /// Requested HTTPS URL.
        url: String,
        /// Parser or transport detail.
        detail: String,
    },
    /// An HTTPS source response exceeded the resolver body cap.
    #[error("tool source `{url}` exceeded {limit} bytes")]
    NetworkTooLarge {
        /// Requested HTTPS URL.
        url: String,
        /// Maximum accepted body size in bytes.
        limit: u64,
        /// Reported or observed body size when available.
        actual: Option<u64>,
    },
    /// An HTTPS source failed for a non-status, non-timeout transport reason.
    #[error("tool source `{url}` network error: {detail}")]
    Network {
        /// Requested HTTPS URL.
        url: String,
        /// Transport detail.
        detail: String,
    },
    /// A unique temporary cache path could not be allocated.
    #[error("tool cache temporary-path collision under {}: {stem}", parent.display())]
    CacheCollision {
        /// Parent directory where the temporary path was attempted.
        parent: PathBuf,
        /// Stable stem used for the temporary path.
        stem: String,
    },
}

impl ToolError {
    /// Build a manifest-read error with path context.
    #[must_use]
    pub const fn manifest_read(path: PathBuf, source: std::io::Error) -> Self {
        Self::ManifestRead { path, source }
    }

    /// Build a manifest-parse error with path context.
    #[must_use]
    pub fn manifest_parse(path: PathBuf, source: serde_saphyr::Error) -> Self {
        Self::ManifestParse {
            path,
            source: Box::new(source),
        }
    }

    #[allow(dead_code, reason = "Chunk 2 introduces cache errors before every call site lands.")]
    pub(crate) fn cache_io(
        action: &'static str, path: impl Into<PathBuf>, source: std::io::Error,
    ) -> Self {
        Self::CacheIo {
            action,
            path: path.into(),
            source,
        }
    }

    /// Build a permission-shape error.
    #[must_use]
    pub fn invalid_permission(template: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidPermission {
            template: template.into(),
            reason: reason.into(),
        }
    }

    /// Build a permission-denied error.
    #[must_use]
    pub fn permission_denied(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::PermissionDenied {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// Build a runtime error.
    #[must_use]
    pub fn runtime(detail: impl Into<String>) -> Self {
        Self::Runtime(detail.into())
    }

    /// Build an invalid-source error.
    #[must_use]
    pub fn invalid_source(source: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidSource {
            source_value: source.into(),
            reason: reason.into(),
        }
    }

    /// Build a local source I/O error.
    #[must_use]
    pub fn source_io(
        action: &'static str, path: impl Into<PathBuf>, source: std::io::Error,
    ) -> Self {
        Self::SourceIo {
            action,
            path: path.into(),
            source,
        }
    }
}

impl From<ToolError> for specify_error::Error {
    fn from(value: ToolError) -> Self {
        match value {
            ToolError::ToolNotDeclared { name } => Self::ToolNotDeclared { name },
            ToolError::Runtime(detail) => Self::Diag {
                code: "tool-runtime",
                detail,
            },
            err @ (ToolError::InvalidPermission { .. } | ToolError::PermissionDenied { .. }) => {
                Self::ToolDenied(err.to_string())
            }
            other => Self::Diag {
                code: "tool-resolver",
                detail: other.to_string(),
            },
        }
    }
}
