//! On-disk `meta.yaml` sidecar — schema, atomic read, atomic write.

use std::path::Path;
use std::{fs, io};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use super::{SIDECAR_FILENAME, scope_segment};
use crate::error::{SidecarKind, ToolError};
use crate::manifest::{ToolPermissions, ToolScope};
use crate::package::PackageMetadata;

const SIDECAR_SCHEMA_VERSION: u32 = 1;

/// Permission metadata captured at fetch time for operator inspection.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PermissionsSnapshot {
    /// Read-only preopen templates from the live declaration.
    #[serde(default)]
    pub read: Vec<String>,
    /// Read-write preopen templates from the live declaration.
    #[serde(default)]
    pub write: Vec<String>,
}

/// Package metadata captured at fetch time for operator inspection.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PackageSnapshot {
    /// Package name without the version suffix.
    pub name: String,
    /// Exact package version.
    pub version: String,
    /// Registry host used to resolve the package.
    pub registry: String,
}

/// OCI metadata captured at fetch time for operator inspection.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OciSnapshot {
    /// Resolved OCI artifact reference when known.
    pub reference: String,
}

impl From<&PackageMetadata> for PackageSnapshot {
    fn from(value: &PackageMetadata) -> Self {
        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            registry: value.registry.clone(),
        }
    }
}

impl From<&ToolPermissions> for PermissionsSnapshot {
    fn from(value: &ToolPermissions) -> Self {
        Self {
            read: value.read.clone(),
            write: value.write.clone(),
        }
    }
}

/// On-disk `meta.yaml` metadata beside cached tool bytes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Sidecar {
    /// Sidecar schema version. Version `1` is the only supported shape.
    pub schema_version: u32,
    /// Scope segment, for example `project--my-app`.
    pub scope: String,
    /// Tool name from the declaration.
    pub tool_name: String,
    /// Tool version from the declaration.
    pub tool_version: String,
    /// Literal source string from the declaration.
    pub source: String,
    /// UTC timestamp from when the bytes were fetched or copied.
    pub fetched_at: DateTime<Utc>,
    /// Fetch-time permissions snapshot. Informational only.
    pub permissions_snapshot: PermissionsSnapshot,
    /// Optional lower-case hex SHA-256 digest copied from the declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Optional package metadata for wasm-pkg sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageSnapshot>,
    /// Optional OCI metadata for wasm-pkg sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci: Option<OciSnapshot>,
}

impl Sidecar {
    /// Construct a v1 sidecar from a live declaration tuple.
    ///
    /// `now` records the `fetched_at` stamp; the resolver supplies
    /// `Utc::now` and tests pin a deterministic value.
    ///
    /// # Errors
    ///
    /// Returns `ToolError::InvalidCacheSegment` when the scope's project name
    /// or capability slug is empty, contains a path separator, or would escape
    /// the cache directory. Other fields are accepted verbatim and validated
    /// against the v1 schema by [`write_sidecar`] before persistence.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: &ToolScope, tool_name: impl Into<String>, tool_version: impl Into<String>,
        source: impl Into<String>, permissions_snapshot: PermissionsSnapshot,
        sha256: Option<String>, package_metadata: Option<PackageMetadata>, now: DateTime<Utc>,
    ) -> Result<Self, ToolError> {
        let (package, oci) = package_metadata.map_or((None, None), |metadata| {
            let package = Some(PackageSnapshot::from(&metadata));
            let oci = metadata.oci_reference.map(|reference| OciSnapshot { reference });
            (package, oci)
        });
        Ok(Self {
            schema_version: SIDECAR_SCHEMA_VERSION,
            scope: scope_segment(scope)?,
            tool_name: tool_name.into(),
            tool_version: tool_version.into(),
            source: source.into(),
            fetched_at: now,
            permissions_snapshot,
            sha256,
            package,
            oci,
        })
    }
}

/// Read `meta.yaml` from `path`.
///
/// Missing sidecars are returned as `Ok(None)` so callers can distinguish a
/// cold cache from a corrupt one and treat the former as a [`super::Status::MissNotFound`].
///
/// # Errors
///
/// Returns `ToolError::Io` when the file exists but cannot be read,
/// `ToolError::Sidecar { kind: SidecarKind::Parse, .. }` when the bytes are
/// not valid YAML or do not deserialize into the v1 shape, and
/// `ToolError::Sidecar { kind: SidecarKind::Schema, .. }` when the parsed
/// document violates a schema invariant (`schema-version != 1`, an empty
/// required field, or a malformed `sha256` digest).
pub fn read_sidecar(path: &Path) -> Result<Option<Sidecar>, ToolError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(ToolError::cache_io("read sidecar", path, err)),
    };
    let sidecar: Sidecar = serde_saphyr::from_str(&contents).map_err(|err| ToolError::Sidecar {
        path: path.to_path_buf(),
        kind: SidecarKind::Parse(Box::new(err.into())),
    })?;
    validate_sidecar_schema(path, &sidecar)?;
    Ok(Some(sidecar))
}

/// Write `meta.yaml` to `path` via a sibling temporary file and atomic rename.
///
/// # Errors
///
/// Returns `ToolError::Sidecar { kind: SidecarKind::Schema, .. }` when
/// `sidecar` does not satisfy the v1 schema, `ToolError::CacheRoot` when
/// `path` has no parent directory, `ToolError::Io` when the parent directory
/// cannot be created, a unique temp path cannot be allocated, or the temp
/// file cannot be written, and `ToolError::AtomicMoveFailed` when the final
/// rename into place fails (a crash here leaves the destination untouched).
pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), ToolError> {
    validate_sidecar_schema(path, sidecar)?;
    let Some(parent) = path.parent() else {
        return Err(ToolError::CacheRoot(format!(
            "sidecar path has no parent: {}",
            path.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create sidecar parent", parent, err))?;
    let contents = serde_saphyr::to_string(sidecar).map_err(|err| ToolError::Sidecar {
        path: path.to_path_buf(),
        kind: SidecarKind::Schema(format!("failed to serialize sidecar: {err}")),
    })?;
    let prefix = format!(".{SIDECAR_FILENAME}.");
    let tmp = Builder::new()
        .prefix(&prefix)
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|err| ToolError::cache_io("create sidecar temp", parent, err))?;
    fs::write(tmp.path(), contents)
        .map_err(|err| ToolError::cache_io("write sidecar temp", tmp.path(), err))?;
    tmp.persist(path).map(|_| ()).map_err(|err| ToolError::AtomicMoveFailed {
        from: err.file.path().to_path_buf(),
        to: path.to_path_buf(),
        source: err.error,
    })
}

fn validate_sidecar_schema(path: &Path, sidecar: &Sidecar) -> Result<(), ToolError> {
    if sidecar.schema_version != SIDECAR_SCHEMA_VERSION {
        return sidecar_schema_error(path, "schema-version must be 1");
    }
    for (field, value) in [
        ("scope", sidecar.scope.as_str()),
        ("tool-name", sidecar.tool_name.as_str()),
        ("tool-version", sidecar.tool_version.as_str()),
        ("source", sidecar.source.as_str()),
    ] {
        if value.is_empty() {
            return sidecar_schema_error(path, format!("{field} must not be empty"));
        }
    }
    if let Some(sha256) = &sidecar.sha256
        && !valid_sha256(sha256)
    {
        return sidecar_schema_error(path, "sha256 must be 64 lowercase hexadecimal characters");
    }
    if let Some(package) = &sidecar.package {
        for (field, value) in [
            ("package.name", package.name.as_str()),
            ("package.version", package.version.as_str()),
            ("package.registry", package.registry.as_str()),
        ] {
            if value.is_empty() {
                return sidecar_schema_error(path, format!("{field} must not be empty"));
            }
        }
    }
    if let Some(oci) = &sidecar.oci
        && oci.reference.is_empty()
    {
        return sidecar_schema_error(path, "oci.reference must not be empty");
    }
    Ok(())
}

fn sidecar_schema_error(path: &Path, detail: impl Into<String>) -> Result<(), ToolError> {
    Err(ToolError::Sidecar {
        path: path.to_path_buf(),
        kind: SidecarKind::Schema(detail.into()),
    })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
