//! Tool-specific errors and conversions into the shared Specify error surface.

use std::path::PathBuf;

use specify_error::YamlError;

/// Errors produced by declared-tool manifest loading, validation, and cache
/// helpers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[allow(missing_docs, reason = "field names are self-evident; variant docs carry the contract")]
pub enum ToolError {
    /// `tools.yaml` could not be read or parsed.
    #[error("tool manifest at {}: {kind}", path.display())]
    Manifest {
        path: PathBuf,
        #[source]
        kind: ManifestKind,
    },
    /// A deterministic cache root could not be selected from the environment.
    #[error("tool cache root error: {0}")]
    CacheRoot(String),
    /// A filesystem operation failed against a cache or local-source path.
    #[error("tool I/O failed while {action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// `meta.yaml` could not be parsed or violated the sidecar schema.
    #[error("tool sidecar at {}: {kind}", path.display())]
    Sidecar {
        path: PathBuf,
        #[source]
        kind: SidecarKind,
    },
    /// An atomic cache install or replacement step failed.
    #[error("atomic cache move failed from {} to {}: {source}", from.display(), to.display())]
    AtomicMoveFailed {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The requested tool name was not present in merged declarations.
    #[error("tool not declared: {name}")]
    ToolNotDeclared { name: String },
    /// Two declaration sites declared the same tool name.
    #[error("tool name collision: {name}")]
    ToolNameCollision { name: String },
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
    /// The CLI was compiled without the `host` Cargo feature, so the WASI
    /// runner is a stub. Surfaces as the `tool-host-not-built` diagnostic.
    #[error(
        "tool host runtime not built: this build of the `specify` CLI was compiled without the `host` feature; rebuild with `--features host` (or use the default install) to run WASI tools"
    )]
    HostNotBuilt,
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
    /// A wasm-pkg package source could not be resolved or streamed.
    #[error("tool package `{source_value}` failed: {reason}")]
    Package { source_value: String, reason: String },
    /// An HTTPS source request failed (status, timeout, malformed URL, body
    /// cap exceeded, or generic transport failure).
    #[error("tool source `{url}`: {kind}")]
    Network {
        url: String,
        #[source]
        kind: NetworkKind,
    },
    /// A unique temporary cache path could not be allocated.
    #[error("tool cache temporary-path collision under {}: {stem}", parent.display())]
    CacheCollision {
        parent: PathBuf,
        /// Stable stem used for the temporary path.
        stem: String,
    },
    /// A `package:` source was declared, but the CLI was compiled without the
    /// `oci` Cargo feature. Surfaces as the `tool-package-source-disabled`
    /// diagnostic; declared sources still parse so the operator gets a clean
    /// rebuild hint instead of a structural-validation failure.
    #[error(
        "tool package source disabled: this build of the `specify` CLI was compiled without the `oci` feature; rebuild with `--features oci` to resolve `package:` tool sources"
    )]
    PackageDisabled,
}

/// Sub-kind for [`ToolError::Manifest`].
#[derive(Debug, thiserror::Error)]
pub enum ManifestKind {
    /// Reading `tools.yaml` failed.
    #[error("read failed: {0}")]
    Read(#[source] std::io::Error),
    /// Parsing `tools.yaml` failed.
    #[error("parse failed: {0}")]
    Parse(#[source] Box<YamlError>),
}

/// Sub-kind for [`ToolError::Sidecar`].
#[derive(Debug, thiserror::Error)]
pub enum SidecarKind {
    /// `meta.yaml` existed but could not be parsed as YAML.
    #[error("invalid YAML: {0}")]
    Parse(#[source] Box<YamlError>),
    /// `meta.yaml` parsed, but did not satisfy the sidecar schema.
    #[error("invalid schema: {0}")]
    Schema(String),
}

/// Sub-kind for [`ToolError::Network`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs, reason = "field names are self-evident; variant docs carry the contract")]
pub enum NetworkKind {
    /// An HTTPS source returned a non-200 status.
    #[error("returned HTTP status {0}; expected 200")]
    Status(u16),
    /// An HTTPS source timed out.
    #[error("timed out: {0}")]
    Timeout(String),
    /// An HTTPS source URL or response was malformed.
    #[error("is malformed: {0}")]
    Malformed(String),
    /// An HTTPS source response exceeded the resolver body cap.
    #[error("exceeded {limit} bytes")]
    TooLarge {
        limit: u64,
        /// Reported or observed body size when available.
        actual: Option<u64>,
    },
    /// A non-status, non-timeout transport failure.
    #[error("network error: {0}")]
    Other(String),
}

impl ToolError {
    /// Build a cache or local-source I/O error. The single named helper
    /// keeps call sites readable across cache writes, resolver staging,
    /// and local-source reads; all produce the merged [`Self::Io`]
    /// variant.
    pub(crate) fn cache_io(
        action: &'static str, path: impl Into<PathBuf>, source: std::io::Error,
    ) -> Self {
        Self::Io {
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

    #[cfg(feature = "oci")]
    pub(crate) fn package(
        request: &crate::manifest::PackageRequest, reason: impl Into<String>,
    ) -> Self {
        Self::Package {
            source_value: request.to_wire_string(),
            reason: reason.into(),
        }
    }
}

impl From<ToolError> for specify_error::Error {
    fn from(value: ToolError) -> Self {
        let code = match &value {
            ToolError::ToolNotDeclared { .. } => "tool-not-declared",
            ToolError::Runtime(_) => "tool-runtime",
            ToolError::HostNotBuilt => "tool-host-not-built",
            ToolError::PackageDisabled => "tool-package-source-disabled",
            ToolError::InvalidPermission { .. } | ToolError::PermissionDenied { .. } => {
                "tool-permission-denied"
            }
            _ => "tool-resolver",
        };
        Self::Diag {
            code,
            detail: value.to_string(),
        }
    }
}
