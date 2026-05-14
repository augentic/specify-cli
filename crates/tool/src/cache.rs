//! Global cache layout and metadata helpers for resolved WASI tools.
//! Owns pure path/scope helpers and the [`Status`] decision API;
//! I/O concerns live in [`fetch`], [`gc`], and [`meta`].

use std::path::{Component, Path, PathBuf};
use std::{env, fs, io};

use crate::error::ToolError;
use crate::manifest::ToolScope;

pub mod fetch;
pub mod gc;
pub mod meta;

#[cfg(test)]
mod tests;

pub use fetch::stage_and_install;
pub use gc::scan as scan_for_gc;
pub(crate) use meta::SIDECAR_SCHEMA_VERSION;
pub use meta::{Sidecar, read_sidecar, write_sidecar};

/// Filename used for cached component bytes.
pub const MODULE_FILENAME: &str = "module.wasm";

/// Filename used for cached tool metadata.
pub const SIDECAR_FILENAME: &str = "meta.yaml";

/// Embedded JSON Schema for cache sidecars.
pub const TOOL_SIDECAR_JSON_SCHEMA: &str = include_str!("../schemas/tool-sidecar.schema.json");

/// Cache reuse state for a declared tool.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Status {
    /// Cached bytes and sidecar metadata match the live declaration tuple.
    Hit,
    /// The cache directory, module, or sidecar is absent.
    MissNotFound,
    /// Cached metadata exists but no longer matches the live declaration tuple.
    MissChanged,
}

/// Resolve the global tool cache root.
///
/// Precedence is `SPECIFY_TOOLS_CACHE`, then `XDG_CACHE_HOME/specify/tools`,
/// then `$HOME/.cache/specify/tools`.
///
/// # Errors
///
/// Returns the `tool-cache-root` diagnostic when an explicit
/// `SPECIFY_TOOLS_CACHE` or `XDG_CACHE_HOME` override is set but empty or
/// relative, when `HOME` is set but empty or relative, or when none of
/// the three precedence variables is set and so no fallback root can be
/// selected.
pub fn root() -> Result<PathBuf, ToolError> {
    if let Some(value) = env::var_os("SPECIFY_TOOLS_CACHE") {
        return env_path("SPECIFY_TOOLS_CACHE", value);
    }

    if let Some(value) = env::var_os("XDG_CACHE_HOME") {
        return env_path("XDG_CACHE_HOME", value).map(|root| root.join("specify").join("tools"));
    }

    if let Some(home) = env::var_os("HOME") {
        let home = env_path("HOME", home)?;
        return Ok(home.join(".cache").join("specify").join("tools"));
    }

    Err(ToolError::cache_root(
        "could not determine a cache directory from SPECIFY_TOOLS_CACHE, XDG_CACHE_HOME, or HOME",
    ))
}

/// Convert a declaration scope into the on-disk scope segment.
///
/// # Errors
///
/// Returns `ToolError::InvalidCacheSegment` when the project name or
/// capability slug is empty, contains a path separator, equals `.` or `..`,
/// or contains a component that would escape the scope directory.
pub fn scope_segment(scope: &ToolScope) -> Result<String, ToolError> {
    match scope {
        ToolScope::Project { project_name } => {
            validate_segment("project name", project_name)?;
            Ok(format!("project--{project_name}"))
        }
        ToolScope::Capability { capability_slug, .. } => {
            validate_segment("capability slug", capability_slug)?;
            Ok(format!("capability--{capability_slug}"))
        }
    }
}

/// Compute the cache directory for one tool version.
///
/// # Errors
///
/// Returns the `tool-cache-root` diagnostic when no cache root can be
/// selected from the environment, and `ToolError::InvalidCacheSegment`
/// when `name`, `version`, or the scope's project/capability slug fails
/// segment validation.
pub fn tool_dir(scope: &ToolScope, name: &str, version: &str) -> Result<PathBuf, ToolError> {
    validate_segment("tool name", name)?;
    validate_segment("tool version", version)?;
    Ok(root()?.join(scope_segment(scope)?).join(name).join(version))
}

/// Compute the cached module path for one tool version.
///
/// # Errors
///
/// Forwards every error returned by [`tool_dir`].
pub fn module_path(scope: &ToolScope, name: &str, version: &str) -> Result<PathBuf, ToolError> {
    Ok(tool_dir(scope, name, version)?.join(MODULE_FILENAME))
}

/// Compute the cached sidecar path for one tool version.
///
/// # Errors
///
/// Forwards every error returned by [`tool_dir`].
pub fn sidecar_path(scope: &ToolScope, name: &str, version: &str) -> Result<PathBuf, ToolError> {
    Ok(tool_dir(scope, name, version)?.join(SIDECAR_FILENAME))
}

/// Return cache status for a live declaration tuple.
///
/// # Errors
///
/// Forwards every error returned by [`module_path()`], [`sidecar_path`], and
/// [`read_sidecar`] — the latter surfaces the `tool-io` diagnostic and
/// the `tool-sidecar-parse` / `tool-sidecar-schema` diagnostics when an
/// existing `meta.yaml` is unreadable or malformed (a missing sidecar is
/// reported as [`Status::MissNotFound`] rather than as an error).
pub fn status(
    scope: &ToolScope, tool_name: &str, tool_version: &str, source: &str, sha256: Option<&str>,
) -> Result<Status, ToolError> {
    let module = module_path(scope, tool_name, tool_version)?;
    if !module.is_file() {
        return Ok(Status::MissNotFound);
    }
    let Some(sidecar) = read_sidecar(&sidecar_path(scope, tool_name, tool_version)?)? else {
        return Ok(Status::MissNotFound);
    };
    sidecar_status(scope, tool_name, tool_version, source, sha256, &sidecar)
}

/// Compare an already-loaded sidecar against a live declaration tuple.
///
/// # Errors
///
/// Returns `ToolError::InvalidCacheSegment` when the live scope cannot be
/// converted into a valid cache segment.
pub fn sidecar_status(
    scope: &ToolScope, tool_name: &str, tool_version: &str, source: &str, sha256: Option<&str>,
    sidecar: &Sidecar,
) -> Result<Status, ToolError> {
    let live_scope = scope_segment(scope)?;
    if sidecar.scope == live_scope
        && sidecar.tool_name == tool_name
        && sidecar.tool_version == tool_version
        && sidecar.source == source
        && sidecar.sha256.as_deref() == sha256
    {
        Ok(Status::Hit)
    } else {
        Ok(Status::MissChanged)
    }
}

fn env_path(name: &'static str, value: std::ffi::OsString) -> Result<PathBuf, ToolError> {
    if value.is_empty() {
        return Err(ToolError::cache_root(format!("{name} is set but empty")));
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ToolError::cache_root(format!(
            "{name} must be an absolute path: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn validate_segment(field: &'static str, value: &str) -> Result<(), ToolError> {
    if value.is_empty() {
        return Err(invalid_segment(field, value, "must not be empty"));
    }
    if value.contains('/') || value.contains('\\') {
        return Err(invalid_segment(field, value, "must not contain path separators"));
    }
    if value == "." || value == ".." {
        return Err(invalid_segment(field, value, "must not be a relative path segment"));
    }
    if Path::new(value).components().any(|component| {
        matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
    }) {
        return Err(invalid_segment(field, value, "must stay within the cache directory"));
    }
    Ok(())
}

fn invalid_segment(field: &'static str, value: &str, reason: &'static str) -> ToolError {
    ToolError::InvalidCacheSegment {
        field,
        value: value.to_string(),
        reason,
    }
}

fn sorted_dir_entries(path: &Path) -> Result<Vec<fs::DirEntry>, ToolError> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(path)
        .map_err(|err| ToolError::cache_io("read cache directory", path, err))?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|err| ToolError::cache_io("read cache directory entry", path, err))?;
    entries.sort_by_key(fs::DirEntry::path);
    Ok(entries)
}
