//! On-disk `meta.yaml` sidecar — schema, atomic read, atomic write.

use std::path::Path;
use std::{fs, io};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{SIDECAR_FILENAME, scope_segment, unique_sibling_path};
use crate::error::ToolError;
use crate::manifest::{ToolPermissions, ToolScope};

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
}

impl Sidecar {
    /// Construct a v1 sidecar from a live declaration tuple.
    ///
    /// # Errors
    ///
    /// Returns `ToolError::InvalidCacheSegment` when the scope's project name
    /// or capability slug is empty, contains a path separator, or would escape
    /// the cache directory. Other fields are accepted verbatim and validated
    /// against the v1 schema by [`write_sidecar`] before persistence.
    pub fn new(
        scope: &ToolScope, tool_name: impl Into<String>, tool_version: impl Into<String>,
        source: impl Into<String>, permissions_snapshot: PermissionsSnapshot,
        sha256: Option<String>,
    ) -> Result<Self, ToolError> {
        Ok(Self {
            schema_version: SIDECAR_SCHEMA_VERSION,
            scope: scope_segment(scope)?,
            tool_name: tool_name.into(),
            tool_version: tool_version.into(),
            source: source.into(),
            fetched_at: Utc::now(),
            permissions_snapshot,
            sha256,
        })
    }
}

/// Read `meta.yaml` from `path`.
///
/// Missing sidecars are returned as `Ok(None)` so callers can distinguish a
/// cold cache from a corrupt one and treat the former as a [`super::CacheStatus::MissNotFound`].
///
/// # Errors
///
/// Returns `ToolError::CacheIo` when the file exists but cannot be read,
/// `ToolError::SidecarParse` when the bytes are not valid YAML or do not
/// deserialize into the v1 shape, and `ToolError::SidecarSchema` when the
/// parsed document violates a schema invariant (`schema-version != 1`, an
/// empty required field, or a malformed `sha256` digest).
pub fn read_sidecar(path: &Path) -> Result<Option<Sidecar>, ToolError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(ToolError::cache_io("read sidecar", path, err)),
    };
    let sidecar: Sidecar =
        serde_saphyr::from_str(&contents).map_err(|err| ToolError::SidecarParse {
            path: path.to_path_buf(),
            source: Box::new(err),
        })?;
    validate_sidecar_schema(path, &sidecar)?;
    Ok(Some(sidecar))
}

/// Write `meta.yaml` to `path` via a sibling temporary file and atomic rename.
///
/// # Errors
///
/// Returns `ToolError::SidecarSchema` when `sidecar` does not satisfy the v1
/// schema, `ToolError::CacheRoot` when `path` has no parent directory,
/// `ToolError::CacheIo` when the parent directory cannot be created or the
/// temp file cannot be written, `ToolError::CacheCollision` when a unique
/// temp path could not be picked after the configured maximum retries,
/// and `ToolError::AtomicMoveFailed` when the final rename into place fails
/// (a crash here leaves the destination untouched).
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
    let contents = serde_saphyr::to_string(sidecar).map_err(|err| ToolError::SidecarSchema {
        path: path.to_path_buf(),
        detail: format!("failed to serialize sidecar: {err}"),
    })?;
    let tmp = unique_sibling_path(parent, SIDECAR_FILENAME)?;
    fs::write(&tmp, contents)
        .map_err(|err| ToolError::cache_io("write sidecar temp", &tmp, err))?;
    fs::rename(&tmp, path).map_err(|err| ToolError::AtomicMoveFailed {
        from: tmp,
        to: path.to_path_buf(),
        source: err,
    })?;
    Ok(())
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
    Ok(())
}

fn sidecar_schema_error(path: &Path, detail: impl Into<String>) -> Result<(), ToolError> {
    Err(ToolError::SidecarSchema {
        path: path.to_path_buf(),
        detail: detail.into(),
    })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
