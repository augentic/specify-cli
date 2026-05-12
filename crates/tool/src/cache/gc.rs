//! Orphan-cache scan for `specify tool gc` — computes the cache
//! `<scope>/<tool>/<version>/` directories that are no longer
//! referenced by the live merged manifest.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::hash::BuildHasher;
use std::path::PathBuf;

use super::{SIDECAR_FILENAME, read_sidecar, root, scope_segment, sorted_dir_entries};
use crate::error::ToolError;
use crate::manifest::ToolScope;

/// Return version directories under `scope` not referenced by `kept`.
///
/// The keep-set tuple is `(tool-name, tool-version, source)`. The scan is
/// limited to the supplied scope segment; another project or capability with
/// the same tool name is not considered. The returned vector is deduplicated
/// by the directory layout (one entry per `<tool>/<version>/` directory) and
/// sorted for deterministic output.
///
/// # Errors
///
/// Returns `ToolError::CacheRoot` when the cache root cannot be selected,
/// `ToolError::InvalidCacheSegment` when a discovered directory name is not
/// valid UTF-8 or violates the cache-segment invariants, `ToolError::Io`
/// when the scope or tool directory cannot be read, and the `ToolError::Sidecar`
/// parse/schema variants when an existing `meta.yaml` is malformed (a
/// missing sidecar marks the directory as unreferenced rather than erroring).
pub fn scan<S: BuildHasher>(
    scope: &ToolScope, kept: &HashSet<(String, String, String), S>,
) -> Result<Vec<PathBuf>, ToolError> {
    let scope_dir = root()?.join(scope_segment(scope)?);
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

fn file_name_string(path: &std::path::Path, field: &'static str) -> Result<String, ToolError> {
    path.file_name().and_then(OsStr::to_str).map(ToOwned::to_owned).ok_or_else(|| {
        ToolError::InvalidCacheSegment {
            field,
            value: path.display().to_string(),
            reason: "must be valid UTF-8",
        }
    })
}
