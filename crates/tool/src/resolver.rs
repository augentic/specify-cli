//! Source resolution for local paths, `file:` URIs, and `https:` URIs.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::NamedTempFile;

pub mod digest;
pub mod http;
pub mod local;

use crate::cache::{
    self, CacheStatus, MODULE_FILENAME, PermissionsSnapshot, SIDECAR_FILENAME, Sidecar,
};
use crate::error::ToolError;
use crate::manifest::{Tool, ToolScope, ToolSource};

const MAX_TEMP_ATTEMPTS: u8 = 64;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Cached component bytes plus the live declaration data needed to run them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// Path to the cached WASI component bytes.
    pub bytes_path: PathBuf,
    /// Declaration site that supplied the tool.
    pub scope: ToolScope,
    /// Live manifest declaration used for argv and permission evaluation.
    pub tool: Tool,
}

/// Resolve a declared tool source into the global immutable cache.
///
/// A cache hit is valid only when the live declaration tuple matches the
/// sidecar and, when `sha256` is pinned, the cached bytes still hash to that
/// digest. Misses and digest refreshes stage `module.wasm` and `meta.yaml`
/// together, then atomically install the complete version directory.
///
/// `now` records the sidecar `fetched_at`; the dispatcher passes
/// `Utc::now`, tests pin a deterministic stamp.
///
/// # Errors
///
/// Returns cache errors, source read errors, digest mismatches, or typed network
/// resolver errors.
pub fn resolve(
    scope: &ToolScope, tool: &Tool, now: chrono::DateTime<chrono::Utc>,
) -> Result<ResolvedTool, ToolError> {
    let source = tool.source.to_wire_string().into_owned();
    let module = cache::module_path(scope, &tool.name, &tool.version)?;
    if cache::cache_status(scope, &tool.name, &tool.version, &source, tool.sha256.as_deref())?
        == CacheStatus::Hit
        && digest::cached_matches(&module, tool.sha256.as_deref())?
    {
        return Ok(resolved(scope, tool, module));
    }

    let dest = cache::tool_dir(scope, &tool.name, &tool.version)?;
    let staged = unique_staging_dir(&dest)?;
    let install_result = stage_and_install(scope, tool, &source, &staged, &dest, now);
    // The atomic install moves `staged/` into `dest/`, so its absence on
    // success is expected. On failure we tear down the staging tree.
    let cleanup_result = if install_result.is_ok() && !staged.exists() {
        Ok(())
    } else {
        fs::remove_dir_all(&staged)
    };
    match (install_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(resolved(scope, tool, dest.join(MODULE_FILENAME))),
        (Ok(()), Err(err)) => {
            Err(ToolError::cache_io("remove resolver staging directory", staged, err))
        }
        (Err(err), _) => Err(err),
    }
}

fn stage_and_install(
    scope: &ToolScope, tool: &Tool, source: &str, staged: &Path, dest: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ToolError> {
    let module_dest = staged.join(MODULE_FILENAME);
    let acquired = acquire_source_bytes(&tool.source, &module_dest)?;
    digest::validate(source, &acquired, tool.sha256.as_deref())?;
    acquired.persist_to(&module_dest)?;
    let sidecar = Sidecar::new(
        scope,
        &tool.name,
        &tool.version,
        source,
        PermissionsSnapshot::from(&tool.permissions),
        tool.sha256.clone(),
        now,
    )?;
    cache::write_sidecar(&staged.join(SIDECAR_FILENAME), &sidecar)?;
    cache::stage_and_install(staged, dest)
}

fn resolved(scope: &ToolScope, tool: &Tool, bytes_path: PathBuf) -> ResolvedTool {
    ResolvedTool {
        bytes_path,
        scope: scope.clone(),
        tool: tool.clone(),
    }
}

fn acquire_source_bytes(source: &ToolSource, dest_hint: &Path) -> Result<AcquiredBytes, ToolError> {
    match source {
        ToolSource::LocalPath(path) => {
            local::read_local_path(path, &path.to_string_lossy()).map(AcquiredBytes::Buffered)
        }
        ToolSource::FileUri(uri) => local::read_file_uri(uri).map(AcquiredBytes::Buffered),
        ToolSource::HttpsUri(url) => http::download_https(url, dest_hint),
    }
}

/// Bytes acquired from a tool source, ready for digest validation and
/// installation into the cache. HTTPS streams to a sibling `NamedTempFile`
/// (so the bytes never live in a `Vec`); local sources read into memory
/// because their bodies are bounded by the on-disk source file.
#[derive(Debug)]
pub(crate) enum AcquiredBytes {
    Buffered(Vec<u8>),
    Streamed { temp: NamedTempFile, sha256: String },
}

impl AcquiredBytes {
    pub(crate) fn len(&self) -> Result<u64, ToolError> {
        match self {
            Self::Buffered(bytes) => Ok(bytes.len() as u64),
            Self::Streamed { temp, .. } => temp
                .as_file()
                .metadata()
                .map(|m| m.len())
                .map_err(|err| ToolError::cache_io("stat staged tool body", temp.path(), err)),
        }
    }

    pub(crate) fn sha256_hex(&self) -> String {
        match self {
            Self::Buffered(bytes) => digest::sha256_hex(bytes),
            Self::Streamed { sha256, .. } => sha256.clone(),
        }
    }

    pub(crate) fn persist_to(self, dest: &Path) -> Result<(), ToolError> {
        match self {
            Self::Buffered(bytes) => fs::write(dest, bytes)
                .map_err(|err| ToolError::cache_io("write staged module", dest, err)),
            Self::Streamed { temp, .. } => {
                temp.persist(dest).map(|_| ()).map_err(|err| ToolError::AtomicMoveFailed {
                    from: err.file.path().to_path_buf(),
                    to: dest.to_path_buf(),
                    source: err.error,
                })
            }
        }
    }
}

fn unique_staging_dir(dest: &Path) -> Result<PathBuf, ToolError> {
    let Some(parent) = dest.parent() else {
        return Err(ToolError::CacheRoot(format!(
            "tool cache destination has no parent: {}",
            dest.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create resolver staging parent", parent, err))?;

    let stem = dest.file_name().unwrap_or_else(|| OsStr::new("tool")).to_string_lossy();
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate =
            parent.join(format!(".resolver-{stem}.{}.{}.{}.tmp", std::process::id(), nanos, n));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(ToolError::cache_io(
                    "create resolver staging directory",
                    candidate,
                    err,
                ));
            }
        }
    }
    Err(ToolError::CacheCollision {
        parent: parent.to_path_buf(),
        stem: stem.into_owned(),
    })
}

#[cfg(test)]
pub(super) mod tests_common {
    use std::path::{Path, PathBuf};

    use chrono::{DateTime, Utc};

    use crate::cache;
    use crate::manifest::{Tool, ToolPermissions, ToolScope, ToolSource};

    pub(crate) fn fixed_now() -> DateTime<Utc> {
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
    }

    pub(crate) fn project_scope() -> ToolScope {
        ToolScope::Project {
            project_name: "demo".to_string(),
        }
    }

    pub(crate) fn capability_scope(root: &Path) -> ToolScope {
        ToolScope::Capability {
            capability_slug: "contracts".to_string(),
            capability_dir: root.to_path_buf(),
        }
    }

    pub(crate) fn tool(source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: "contract".to_string(),
            version: "1.0.0".to_string(),
            source,
            sha256,
            permissions: ToolPermissions::default(),
        }
    }

    pub(crate) fn named_tool(name: &str, source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: name.to_string(),
            ..tool(source, sha256)
        }
    }

    pub(crate) fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, bytes).expect("write source");
        path
    }

    pub(crate) fn cached_bytes(scope: &ToolScope, tool: &Tool) -> Vec<u8> {
        std::fs::read(cache::module_path(scope, &tool.name, &tool.version).expect("module path"))
            .expect("read cached module")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::tests_common::*;
    use super::*;
    use crate::manifest::ToolSource;
    use crate::test_support::{scratch_dir, with_cache_env};

    #[test]
    fn local_path_cache_miss_hit_and_source_change() {
        let cache_dir = scratch_dir("resolver-local-cache");
        let source_dir = scratch_dir("resolver-local-source");
        let first = write_source(&source_dir, "first.wasm", b"first");
        let second = write_source(&source_dir, "second.wasm", b"second");
        let scope = project_scope();
        let first_tool = tool(ToolSource::LocalPath(first.clone()), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved = resolve(&scope, &first_tool, fixed_now()).expect("cache miss resolves");
            assert_eq!(fs::read(&resolved.bytes_path).expect("cached bytes"), b"first");

            fs::write(&first, b"changed-at-source").expect("mutate source");
            let hit = resolve(&scope, &first_tool, fixed_now()).expect("cache hit resolves");
            assert_eq!(hit.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &first_tool), b"first");

            let changed_tool = tool(ToolSource::LocalPath(second), None);
            let changed =
                resolve(&scope, &changed_tool, fixed_now()).expect("changed source re-stages");
            assert_eq!(changed.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &changed_tool), b"second");
        });
    }

    #[test]
    fn project_and_capability_scopes_have_isolated_cache_dirs() {
        let cache_dir = scratch_dir("resolver-scope-cache");
        let source_dir = scratch_dir("resolver-scope-source");
        let capability_dir = scratch_dir("resolver-capability");
        let source = write_source(&source_dir, "module.wasm", b"same");
        let project = project_scope();
        let capability = capability_scope(&capability_dir);
        let declared = tool(ToolSource::LocalPath(source), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let project_resolved =
                resolve(&project, &declared, fixed_now()).expect("project resolve");
            let capability_resolved =
                resolve(&capability, &declared, fixed_now()).expect("capability resolve");
            assert_ne!(project_resolved.bytes_path, capability_resolved.bytes_path);
            assert!(project_resolved.bytes_path.to_string_lossy().contains("project--demo"));
            assert!(
                capability_resolved.bytes_path.to_string_lossy().contains("capability--contracts")
            );
        });
    }
}
