//! Global cache layout and metadata helpers for resolved WASI tools.
//!
//! No file locks are planned for v1. Two concurrent cold-cache resolutions may
//! both stage bytes; the resolver's atomic install step will make the final
//! cache state deterministic.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::hash::BuildHasher;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, io};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ToolError;
use crate::manifest::{ToolPermissions, ToolScope};

/// Filename used for cached component bytes.
pub const MODULE_FILENAME: &str = "module.wasm";

/// Filename used for cached tool metadata.
pub const SIDECAR_FILENAME: &str = "meta.yaml";

/// Embedded JSON Schema for cache sidecars.
pub const TOOL_SIDECAR_JSON_SCHEMA: &str = include_str!("../schemas/tool-sidecar.schema.json");

const SIDECAR_SCHEMA_VERSION: u32 = 1;
const MAX_TEMP_ATTEMPTS: u8 = 64;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Cache reuse state for a declared tool.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Cached bytes and sidecar metadata match the live declaration tuple.
    Hit,
    /// The cache directory, module, or sidecar is absent.
    MissNotFound,
    /// Cached metadata exists but no longer matches the live declaration tuple.
    MissChanged,
}

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
    /// Returns an error when the scope cannot be converted into a valid cache
    /// segment.
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

/// Resolve the global tool cache root.
///
/// Precedence is `SPECIFY_TOOLS_CACHE`, then `XDG_CACHE_HOME/specify/tools`,
/// then `dirs::cache_dir()/specify/tools`, with `$HOME/.cache/specify/tools`
/// as a POSIX fallback if `dirs` cannot locate a cache directory.
///
/// # Errors
///
/// Returns an error when an explicit environment override is empty or relative,
/// or no fallback root can be determined.
pub fn cache_root() -> Result<PathBuf, ToolError> {
    if let Some(value) = env::var_os("SPECIFY_TOOLS_CACHE") {
        return env_path("SPECIFY_TOOLS_CACHE", value);
    }

    if let Some(value) = env::var_os("XDG_CACHE_HOME") {
        return env_path("XDG_CACHE_HOME", value).map(|root| root.join("specify").join("tools"));
    }

    if let Some(root) = dirs::cache_dir()
        && root.is_absolute()
    {
        return Ok(root.join("specify").join("tools"));
    }

    if let Some(home) = env::var_os("HOME") {
        let home = env_path("HOME", home)?;
        return Ok(home.join(".cache").join("specify").join("tools"));
    }

    Err(ToolError::CacheRoot(
        "could not determine a cache directory from SPECIFY_TOOLS_CACHE, XDG_CACHE_HOME, dirs::cache_dir, or HOME"
            .to_string(),
    ))
}

/// Convert a declaration scope into the on-disk scope segment.
///
/// # Errors
///
/// Returns an error when the project name or capability slug is empty or would
/// escape the scope directory.
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
/// Returns an error when the cache root cannot be selected or any path segment
/// is invalid.
pub fn tool_dir(scope: &ToolScope, name: &str, version: &str) -> Result<PathBuf, ToolError> {
    validate_segment("tool name", name)?;
    validate_segment("tool version", version)?;
    Ok(cache_root()?.join(scope_segment(scope)?).join(name).join(version))
}

/// Compute the cached module path for one tool version.
///
/// # Errors
///
/// Returns an error when [`tool_dir`] cannot compute the version directory.
pub fn module_path(scope: &ToolScope, name: &str, version: &str) -> Result<PathBuf, ToolError> {
    Ok(tool_dir(scope, name, version)?.join(MODULE_FILENAME))
}

/// Compute the cached sidecar path for one tool version.
///
/// # Errors
///
/// Returns an error when [`tool_dir`] cannot compute the version directory.
pub fn sidecar_path(scope: &ToolScope, name: &str, version: &str) -> Result<PathBuf, ToolError> {
    Ok(tool_dir(scope, name, version)?.join(SIDECAR_FILENAME))
}

/// Read `meta.yaml`.
///
/// Missing sidecars are returned as `Ok(None)` so callers can distinguish a
/// cold cache from a corrupt cache.
///
/// # Errors
///
/// Returns an error when the file exists but cannot be read, parsed, or
/// validated.
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

/// Write `meta.yaml` using a sibling temporary file and atomic rename.
///
/// # Errors
///
/// Returns an error when the sidecar does not satisfy v1 schema requirements,
/// serialization fails, or the filesystem write/rename fails.
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

/// Return cache status for a live declaration tuple.
///
/// # Errors
///
/// Returns an error when the cache paths cannot be computed or an existing
/// sidecar cannot be parsed.
pub fn cache_status(
    scope: &ToolScope, tool_name: &str, tool_version: &str, source: &str, sha256: Option<&str>,
) -> Result<CacheStatus, ToolError> {
    let module = module_path(scope, tool_name, tool_version)?;
    if !module.is_file() {
        return Ok(CacheStatus::MissNotFound);
    }
    let Some(sidecar) = read_sidecar(&sidecar_path(scope, tool_name, tool_version)?)? else {
        return Ok(CacheStatus::MissNotFound);
    };
    sidecar_cache_status(scope, tool_name, tool_version, source, sha256, &sidecar)
}

/// Compare an already-loaded sidecar against a live declaration tuple.
///
/// # Errors
///
/// Returns an error when the live scope cannot be converted into a valid cache
/// segment.
pub fn sidecar_cache_status(
    scope: &ToolScope, tool_name: &str, tool_version: &str, source: &str, sha256: Option<&str>,
    sidecar: &Sidecar,
) -> Result<CacheStatus, ToolError> {
    let live_scope = scope_segment(scope)?;
    if sidecar.scope == live_scope
        && sidecar.tool_name == tool_name
        && sidecar.tool_version == tool_version
        && sidecar.source == source
        && sidecar.sha256.as_deref() == sha256
    {
        Ok(CacheStatus::Hit)
    } else {
        Ok(CacheStatus::MissChanged)
    }
}

/// Install a staged cache directory into `dest`.
///
/// The staged tree is first copied into a sibling temporary directory. The
/// final switch into place uses `rename`. When replacing an existing cache
/// version, the old directory is first renamed to a sibling backup, then the
/// new complete directory is renamed into place. A crash during replacement can
/// leave the destination absent plus a backup, but never a partially copied
/// destination.
///
/// # Errors
///
/// Returns an error if the staged path is not a directory or any copy/rename
/// operation fails.
pub fn stage_and_install(staged: &Path, dest: &Path) -> Result<(), ToolError> {
    if !staged.is_dir() {
        return Err(ToolError::cache_io(
            "inspect staged directory",
            staged,
            io::Error::new(io::ErrorKind::InvalidInput, "staged path is not a directory"),
        ));
    }
    let Some(parent) = dest.parent() else {
        return Err(ToolError::CacheRoot(format!(
            "destination path has no parent: {}",
            dest.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create cache parent", parent, err))?;

    let install_dir =
        unique_sibling_dir(parent, dest.file_name().unwrap_or_else(|| OsStr::new("tool")))?;
    copy_dir_contents(staged, &install_dir)?;

    let backup = if dest.exists() {
        let backup = unique_sibling_path(parent, ".previous")?;
        fs::rename(dest, &backup).map_err(|err| ToolError::AtomicMoveFailed {
            from: dest.to_path_buf(),
            to: backup.clone(),
            source: err,
        })?;
        Some(backup)
    } else {
        None
    };

    match fs::rename(&install_dir, dest) {
        Ok(()) => {
            if let Some(backup) = backup {
                fs::remove_dir_all(&backup).map_err(|err| {
                    ToolError::cache_io("remove previous cache directory", backup, err)
                })?;
            }
            Ok(())
        }
        Err(source) => {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, dest);
            }
            let _ = fs::remove_dir_all(&install_dir);
            Err(ToolError::AtomicMoveFailed {
                from: install_dir,
                to: dest.to_path_buf(),
                source,
            })
        }
    }
}

/// Find version directories under `scope` that are not referenced by `kept`.
///
/// The keep-set tuple is `(tool-name, tool-version, source)`. The scan is
/// limited to the supplied scope segment; another project or capability with
/// the same tool name is not considered.
///
/// # Errors
///
/// Returns an error when the cache root cannot be selected or an existing
/// sidecar cannot be parsed.
pub fn scan_for_gc<S: BuildHasher>(
    scope: &ToolScope, kept: &HashSet<(String, String, String), S>,
) -> Result<Vec<PathBuf>, ToolError> {
    let scope_dir = cache_root()?.join(scope_segment(scope)?);
    if !scope_dir.exists() {
        return Ok(Vec::new());
    }

    let mut unreferenced = Vec::new();
    for tool_entry in sorted_dir_entries(&scope_dir)? {
        if !tool_entry.path().is_dir() {
            continue;
        }
        let tool_name = file_name_string(&tool_entry.path(), "tool cache directory")?;
        for version_entry in sorted_dir_entries(&tool_entry.path())? {
            let version_dir = version_entry.path();
            if !version_dir.is_dir() {
                continue;
            }
            let version = file_name_string(&version_dir, "tool version directory")?;
            let Some(sidecar) = read_sidecar(&version_dir.join(SIDECAR_FILENAME))? else {
                unreferenced.push(version_dir);
                continue;
            };
            let key = (tool_name.clone(), version, sidecar.source);
            if !kept.contains(&key) || sidecar.scope != scope_segment(scope)? {
                unreferenced.push(version_dir);
            }
        }
    }
    unreferenced.sort();
    Ok(unreferenced)
}

fn env_path(name: &'static str, value: std::ffi::OsString) -> Result<PathBuf, ToolError> {
    if value.is_empty() {
        return Err(ToolError::CacheRoot(format!("{name} is set but empty")));
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ToolError::CacheRoot(format!(
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

fn unique_sibling_path(parent: &Path, stem: impl AsRef<OsStr>) -> Result<PathBuf, ToolError> {
    let stem = stem.as_ref().to_string_lossy();
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{stem}.{}.{}.{}.tmp", std::process::id(), nanos, n));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ToolError::CacheCollision {
        parent: parent.to_path_buf(),
        stem: stem.into_owned(),
    })
}

fn unique_sibling_dir(parent: &Path, stem: impl AsRef<OsStr>) -> Result<PathBuf, ToolError> {
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let candidate = unique_sibling_path(parent, stem.as_ref())?;
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(ToolError::cache_io("create cache temp directory", candidate, err));
            }
        }
    }
    Err(ToolError::CacheCollision {
        parent: parent.to_path_buf(),
        stem: stem.as_ref().to_string_lossy().into_owned(),
    })
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), ToolError> {
    for entry in sorted_dir_entries(src)? {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| ToolError::cache_io("inspect staged entry", &src_path, err))?;
        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)
                .map_err(|err| ToolError::cache_io("create staged subdirectory", &dst_path, err))?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)
                .map_err(|err| ToolError::cache_io("copy staged file", &src_path, err))?;
        } else {
            return Err(ToolError::cache_io(
                "copy staged entry",
                &src_path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staged entries must be files or directories",
                ),
            ));
        }
    }
    Ok(())
}

fn sorted_dir_entries(path: &Path) -> Result<Vec<fs::DirEntry>, ToolError> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(path)
        .map_err(|err| ToolError::cache_io("read cache directory", path, err))?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|err| ToolError::cache_io("read cache directory entry", path, err))?;
    entries.sort_by_key(fs::DirEntry::path);
    Ok(entries)
}

fn file_name_string(path: &Path, field: &'static str) -> Result<String, ToolError> {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid_segment(field, &path.display().to_string(), "must be valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch_dir, with_cache_env};

    fn project_scope() -> ToolScope {
        ToolScope::Project {
            project_name: "demo".to_string(),
        }
    }

    fn capability_scope() -> ToolScope {
        ToolScope::Capability {
            capability_slug: "contracts".to_string(),
            capability_dir: PathBuf::from("/capabilities/contracts"),
        }
    }

    fn fixed_sidecar(scope: &ToolScope, name: &str, version: &str, source: &str) -> Sidecar {
        let mut sidecar = Sidecar::new(
            scope,
            name,
            version,
            source,
            PermissionsSnapshot {
                read: vec!["$PROJECT_DIR/contracts".to_string()],
                write: Vec::new(),
            },
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()),
        )
        .expect("sidecar");
        sidecar.fetched_at = "2026-05-07T00:00:00Z".parse().expect("timestamp");
        sidecar
    }

    fn write_cached_version(scope: &ToolScope, name: &str, version: &str, source: &str) -> PathBuf {
        let dir = tool_dir(scope, name, version).expect("tool dir");
        fs::create_dir_all(&dir).expect("create version dir");
        fs::write(dir.join(MODULE_FILENAME), b"wasm").expect("write module");
        write_sidecar(&dir.join(SIDECAR_FILENAME), &fixed_sidecar(scope, name, version, source))
            .expect("write sidecar");
        dir
    }

    #[test]
    fn cache_root_honours_override_precedence() {
        let override_dir = scratch_dir("override");
        let xdg_dir = scratch_dir("xdg");
        let home_dir = scratch_dir("home");
        with_cache_env(Some(&override_dir), Some(&xdg_dir), Some(&home_dir), || {
            assert_eq!(cache_root().expect("cache root"), override_dir);
        });
    }

    #[test]
    fn cache_root_uses_xdg_before_dirs_fallback() {
        let xdg_dir = scratch_dir("xdg-only");
        let home_dir = scratch_dir("home-only");
        with_cache_env(None, Some(&xdg_dir), Some(&home_dir), || {
            assert_eq!(cache_root().expect("cache root"), xdg_dir.join("specify").join("tools"));
        });
    }

    #[test]
    fn cache_root_uses_dirs_cache_dir_when_no_explicit_env() {
        let home_dir = scratch_dir("dirs-home");
        with_cache_env(None, None, Some(&home_dir), || {
            assert_eq!(
                cache_root().expect("cache root"),
                dirs::cache_dir().expect("dirs cache dir").join("specify").join("tools")
            );
        });
    }

    #[test]
    fn scope_segment_formats_and_rejects_empty_names() {
        assert_eq!(scope_segment(&project_scope()).expect("project segment"), "project--demo");
        assert_eq!(
            scope_segment(&capability_scope()).expect("capability segment"),
            "capability--contracts"
        );
        let empty = ToolScope::Project {
            project_name: String::new(),
        };
        assert!(matches!(scope_segment(&empty), Err(ToolError::InvalidCacheSegment { .. })));
    }

    #[test]
    fn sidecar_round_trips_and_schema_rejects_invalid_shape() {
        let root = scratch_dir("sidecar");
        let path = root.join(SIDECAR_FILENAME);
        let sidecar = fixed_sidecar(
            &project_scope(),
            "contract",
            "1.0.0",
            "https://example.test/contract.wasm",
        );

        write_sidecar(&path, &sidecar).expect("write sidecar");
        assert_eq!(read_sidecar(&path).expect("read sidecar"), Some(sidecar));

        fs::write(
            &path,
            "schema-version: 2\nscope: project--demo\ntool-name: contract\ntool-version: 1.0.0\nsource: https://example.test/contract.wasm\nfetched-at: 2026-05-07T00:00:00Z\npermissions-snapshot:\n  read: []\n  write: []\n",
        )
        .expect("write invalid sidecar");
        assert!(matches!(read_sidecar(&path), Err(ToolError::SidecarSchema { .. })));

        let schema: serde_json::Value =
            serde_json::from_str(TOOL_SIDECAR_JSON_SCHEMA).expect("sidecar schema parses");
        jsonschema::validator_for(&schema).expect("sidecar schema compiles");
    }

    #[test]
    fn cache_status_distinguishes_hit_not_found_and_changed_digest() {
        let cache_dir = scratch_dir("status-cache");
        with_cache_env(Some(&cache_dir), None, None, || {
            assert_eq!(
                cache_status(
                    &project_scope(),
                    "contract",
                    "1.0.0",
                    "https://example.test/contract.wasm",
                    Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                )
                .expect("cold status"),
                CacheStatus::MissNotFound
            );
            write_cached_version(
                &project_scope(),
                "contract",
                "1.0.0",
                "https://example.test/contract.wasm",
            );
            assert_eq!(
                cache_status(
                    &project_scope(),
                    "contract",
                    "1.0.0",
                    "https://example.test/contract.wasm",
                    Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                )
                .expect("hit status"),
                CacheStatus::Hit
            );
            assert_eq!(
                cache_status(
                    &project_scope(),
                    "contract",
                    "1.0.0",
                    "https://example.test/contract.wasm",
                    Some("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
                )
                .expect("changed status"),
                CacheStatus::MissChanged
            );
        });
    }

    #[test]
    fn stage_and_install_installs_complete_tree_and_replaces_existing_version() {
        let root = scratch_dir("stage");
        let staged = root.join("staged");
        let dest = root.join("cache").join("project--demo").join("contract").join("1.0.0");
        fs::create_dir_all(staged.join("nested")).expect("create staged");
        fs::write(staged.join(MODULE_FILENAME), b"new").expect("write module");
        fs::write(staged.join("nested").join("probe.txt"), b"probe").expect("write nested");

        let manual_partial = dest.with_extension("manual-tmp");
        fs::create_dir_all(&manual_partial).expect("create manual temp");
        fs::write(manual_partial.join(MODULE_FILENAME), b"partial").expect("write partial");
        assert!(!dest.exists(), "manual sibling staging must not expose dest");
        fs::remove_dir_all(&manual_partial).expect("remove manual temp");

        stage_and_install(&staged, &dest).expect("install staged");
        assert_eq!(fs::read(dest.join(MODULE_FILENAME)).expect("read module"), b"new");
        assert_eq!(fs::read(dest.join("nested").join("probe.txt")).expect("read nested"), b"probe");

        let staged_replacement = root.join("staged-replacement");
        fs::create_dir_all(&staged_replacement).expect("create replacement");
        fs::write(staged_replacement.join(MODULE_FILENAME), b"replacement")
            .expect("write replacement");
        stage_and_install(&staged_replacement, &dest).expect("replace staged");
        assert_eq!(fs::read(dest.join(MODULE_FILENAME)).expect("read replacement"), b"replacement");
        assert!(!dest.join("nested").exists(), "replacement removes old tree");
    }

    #[test]
    fn scan_for_gc_isolates_scope_and_uses_name_version_source_keep_set() {
        let cache_dir = scratch_dir("gc-cache");
        with_cache_env(Some(&cache_dir), None, None, || {
            let kept_project = write_cached_version(
                &project_scope(),
                "contract",
                "1.0.0",
                "https://example.test/contract.wasm",
            );
            let stale_project = write_cached_version(
                &project_scope(),
                "contract",
                "1.1.0",
                "https://example.test/contract-new.wasm",
            );
            let stale_capability = write_cached_version(
                &capability_scope(),
                "contract",
                "1.0.0",
                "https://example.test/contract.wasm",
            );

            let kept = HashSet::from([(
                "contract".to_string(),
                "1.0.0".to_string(),
                "https://example.test/contract.wasm".to_string(),
            )]);

            let project_gc = scan_for_gc(&project_scope(), &kept).expect("project gc");
            assert_eq!(project_gc, vec![stale_project]);
            assert!(kept_project.exists());

            let capability_gc =
                scan_for_gc(&capability_scope(), &HashSet::new()).expect("capability gc");
            assert_eq!(capability_gc, vec![stale_capability]);
        });
    }
}
