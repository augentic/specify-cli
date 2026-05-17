//! Tool-specific errors and conversions into the shared Specify error surface.

#![allow(
    clippy::needless_pass_by_value,
    reason = "Diag-helper constructors take owned errors so callers can `.map_err(|err| ToolError::cache_io(..., err))` without an extra `&` at every site."
)]

use std::path::PathBuf;

/// Errors produced by declared-tool manifest loading, validation, and cache
/// helpers.
///
/// Variants follow the Diag-first policy from `DECISIONS.md` §"Diag-first
/// error policy": a typed variant exists only when (a) a test or skill
/// destructures the payload, (b) the variant routes to a non-default
/// `Exit` slot via `From<ToolError> for Error`, or (c) three or more call
/// sites share the exact shape. Everything else lands on [`Self::Diag`]
/// with a kebab-case `code` carried at the constructor site (see the
/// `sidecar_*` / `network_*` helpers below).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[expect(missing_docs, reason = "field names are self-evident; variant docs carry the contract")]
pub enum ToolError {
    /// Catch-all diagnostic. The `code` becomes the `error` field of the
    /// JSON envelope after [`From<ToolError> for specify_error::Error`];
    /// `detail` is the human-readable message.
    #[error("{detail}")]
    Diag { code: &'static str, detail: String },
    /// The requested tool name was not present in merged declarations.
    #[error("tool not declared: {name}")]
    ToolNotDeclared { name: String },
    /// A cache path segment would be empty or escape its intended directory.
    #[error("invalid tool cache segment `{value}` for {field}: {reason}")]
    InvalidCacheSegment {
        /// Field being converted to an on-disk path segment.
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    /// A tool permission template or expanded permission path is invalid.
    #[error("invalid tool permission `{template}`: {reason}")]
    InvalidPermission { template: String, reason: String },
    /// A requested preopen or filesystem authority is denied.
    #[error("tool permission denied for {}: {reason}", path.display())]
    PermissionDenied { path: PathBuf, reason: String },
    /// Wasmtime failed to compile, link, instantiate, or run a tool component.
    #[error("tool runtime error: {0}")]
    Runtime(String),
    /// A source declaration is not usable by the resolver.
    #[error("tool source `{source_value}` is invalid: {reason}")]
    InvalidSource { source_value: String, reason: String },
    /// Source bytes were empty.
    #[error("tool source `{source_value}` produced empty bytes")]
    EmptySource { source_value: String },
    /// Source bytes did not match a declared SHA-256 digest.
    #[error("tool source `{source_value}` sha256 mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        source_value: String,
        /// Lowercase hex SHA-256 digest expected from the manifest.
        expected: String,
        /// Lowercase hex SHA-256 digest computed from the fetched bytes.
        actual: String,
    },
}

impl ToolError {
    /// Build a cache or local-source I/O error. The single named helper
    /// keeps call sites readable across cache writes, resolver staging,
    /// and local-source reads.
    pub(crate) fn cache_io(
        action: &'static str, path: impl Into<PathBuf>, source: std::io::Error,
    ) -> Self {
        let path = path.into();
        Self::Diag {
            code: "tool-io",
            detail: format!("tool I/O failed while {action} {}: {source}", path.display()),
        }
    }

    /// Build a cache-root error. Surfaced when an environment-derived
    /// cache root is empty/relative or when a write target has no parent.
    pub(crate) fn cache_root(detail: impl Into<String>) -> Self {
        Self::Diag {
            code: "tool-cache-root",
            detail: format!("tool cache root error: {}", detail.into()),
        }
    }

    /// Build a manifest-read error.
    pub(crate) fn manifest_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        let path = path.into();
        Self::Diag {
            code: "tool-manifest-read",
            detail: format!("tool manifest at {}: read failed: {source}", path.display()),
        }
    }

    /// Build a manifest-parse error.
    pub(crate) fn manifest_parse(path: impl Into<PathBuf>, source: impl std::fmt::Display) -> Self {
        let path = path.into();
        Self::Diag {
            code: "tool-manifest-parse",
            detail: format!("tool manifest at {}: parse failed: {source}", path.display()),
        }
    }

    /// Build an atomic-cache-move error.
    pub(crate) fn atomic_move_failed(
        from: impl Into<PathBuf>, to: impl Into<PathBuf>, source: std::io::Error,
    ) -> Self {
        let from = from.into();
        let to = to.into();
        Self::Diag {
            code: "tool-atomic-move-failed",
            detail: format!(
                "atomic cache move failed from {} to {}: {source}",
                from.display(),
                to.display()
            ),
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

    /// Build a `tool-package` diagnostic for wasm-pkg fetch failures.
    pub(crate) fn package(
        request: &crate::manifest::PackageRequest, reason: impl Into<String>,
    ) -> Self {
        Self::Diag {
            code: "tool-package",
            detail: format!(
                "tool package `{}` failed: {}",
                request.to_wire_string(),
                reason.into()
            ),
        }
    }

    /// Build a `tool-package` diagnostic from a free-form source label.
    /// Used by the wasm-pkg config loader where the originating
    /// `PackageRequest` is no longer in scope.
    pub(crate) fn package_label(label: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Diag {
            code: "tool-package",
            detail: format!("tool package `{}` failed: {}", label.into(), reason.into()),
        }
    }

    /// Build a `tool-sidecar-parse` diagnostic.
    pub(crate) fn sidecar_parse(path: impl Into<PathBuf>, source: impl std::fmt::Display) -> Self {
        let path = path.into();
        Self::Diag {
            code: "tool-sidecar-parse",
            detail: format!("tool sidecar at {}: invalid YAML: {source}", path.display()),
        }
    }

    /// Build a `tool-sidecar-schema` diagnostic.
    pub(crate) fn sidecar_schema(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        let path = path.into();
        Self::Diag {
            code: "tool-sidecar-schema",
            detail: format!(
                "tool sidecar at {}: invalid schema: {}",
                path.display(),
                detail.into()
            ),
        }
    }

    /// Build a `tool-network-status` diagnostic.
    pub(crate) fn network_status(url: impl Into<String>, status: u16) -> Self {
        Self::Diag {
            code: "tool-network-status",
            detail: format!("`{}` returned HTTP status {status}; expected 200", url.into()),
        }
    }

    /// Build a `tool-network-timeout` diagnostic.
    pub(crate) fn network_timeout(url: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Diag {
            code: "tool-network-timeout",
            detail: format!("`{}` timed out: {}", url.into(), detail.into()),
        }
    }

    /// Build a `tool-network-malformed` diagnostic.
    pub(crate) fn network_malformed(url: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Diag {
            code: "tool-network-malformed",
            detail: format!("`{}` is malformed: {}", url.into(), detail.into()),
        }
    }

    /// Build a `tool-network-too-large` diagnostic.
    pub(crate) fn network_too_large(
        url: impl Into<String>, limit: u64, actual: Option<u64>,
    ) -> Self {
        let observed = actual.map_or_else(String::new, |size| format!(" (observed {size} bytes)"));
        Self::Diag {
            code: "tool-network-too-large",
            detail: format!("`{}` exceeded {limit} bytes{observed}", url.into()),
        }
    }

    /// Build a `tool-network-other` diagnostic.
    pub(crate) fn network_other(url: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Diag {
            code: "tool-network-other",
            detail: format!("`{}` network error: {}", url.into(), detail.into()),
        }
    }
}

impl From<ToolError> for specify_error::Error {
    fn from(value: ToolError) -> Self {
        match value {
            ToolError::Diag { code, detail } => Self::Diag { code, detail },
            ToolError::Runtime(detail) => Self::Diag {
                code: "tool-runtime",
                detail,
            },
            err @ ToolError::ToolNotDeclared { .. } => Self::validation_failed(
                "tool-not-declared",
                "tool must be declared in tools.yaml",
                err.to_string(),
            ),
            err @ (ToolError::InvalidPermission { .. } | ToolError::PermissionDenied { .. }) => {
                Self::validation_failed(
                    "tool-permission-denied",
                    "tool must request permitted resources",
                    err.to_string(),
                )
            }
            other => Self::Diag {
                code: "tool-resolver",
                detail: other.to_string(),
            },
        }
    }
}
