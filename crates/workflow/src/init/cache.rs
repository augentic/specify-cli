//! Adapter cache management plus the on-disk
//! `.specify/.cache/.cache-meta.yaml` representation.
//!
//! `cache_adapter` copies a resolved source into the manifest cache at
//! `.specify/.cache/manifests/targets/<name>/` and stamps
//! `cache-meta.yaml` with the resolved URI. The agent owns writes to
//! the manifest cache; the CLI reads `.cache-meta.yaml` (via
//! [`CacheMeta::load`]) only to decide whether the cache matches
//! `.specify/project.yaml:adapter`. The extraction cache at
//! `.specify/.cache/extractions/<adapter>/` lives in a sibling tree and
//! is managed by [`crate::adapter::cache`].

use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::adapter::{Axis, cache_dir as adapter_cache_dir, check_axis_unique_for_name};
use crate::init::adapter_uri::AdapterUri;

/// On-disk metadata describing the contents of `.specify/.cache/`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CacheMeta {
    /// The schema URL or `local:<name>` identifier the cache was populated from.
    pub schema_url: String,
    /// ISO 8601 timestamp of when the cache was last fetched.
    pub fetched_at: String,
}

impl CacheMeta {
    /// Absolute path to `<project_dir>/.specify/.cache/.cache-meta.yaml`.
    #[must_use]
    pub fn path(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify").join(".cache").join(".cache-meta.yaml")
    }
}

/// Copy the resolved adapter source into the project's source/target adapter split
/// axis-aware cache and stamp `.cache-meta.yaml`. Returns the
/// adapter value to record in `project.yaml.adapter`.
pub(super) fn cache_adapter(
    adapter: &str, project_dir: &Path, now: Timestamp,
) -> Result<String, Error> {
    if adapter.trim().is_empty() || adapter != adapter.trim() {
        return Err(Error::Diag {
            code: "adapter-arg-malformed",
            detail: "<adapter> must be non-empty and must not have leading or trailing whitespace"
                .to_string(),
        });
    }

    let source = AdapterUri::parse(adapter, project_dir)?;
    // Cross-axis uniqueness: a target adapter being cached must not
    // collide with an in-repo `adapters/sources/<name>/` (or its
    // cached mirror). See DECISIONS.md §"Adapter name uniqueness".
    // Probing here gives the operator a clean diagnostic before the
    // cache directory is rewritten and ahead of the downstream
    // `TargetAdapter::resolve` call in `init/regular.rs`.
    check_axis_unique_for_name(Axis::Target, &source.adapter_name, project_dir)?;
    let target = adapter_cache_dir(project_dir, Axis::Target, &source.adapter_name);
    refresh_cached_adapter(&source.source_dir, &target)?;
    write_cache_meta(project_dir, &source.adapter_value, now)?;

    Ok(source.adapter_value)
}

fn refresh_cached_adapter(source: &Path, target: &Path) -> Result<(), Error> {
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_dir_recursive(source, target)
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), Error> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn write_cache_meta(project_dir: &Path, adapter_value: &str, now: Timestamp) -> Result<(), Error> {
    let meta = CacheMeta {
        schema_url: adapter_value.to_string(),
        fetched_at: now.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
    };
    let meta_path = CacheMeta::path(project_dir);
    let serialised = serde_saphyr::to_string(&meta)?;
    fs::write(meta_path, serialised)?;
    Ok(())
}
