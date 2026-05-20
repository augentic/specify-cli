//! Adapter cache management: copy the resolved source into
//! `.specify/.cache/<adapter>/`, mirror the bundled `codex` sibling
//! when present, and stamp `cache_meta.yaml` with the resolved URI.

use std::fs;
use std::path::Path;

use jiff::Timestamp;
use specify_error::Error;

use crate::adapter::{CacheMeta, DEFAULT_CODEX_ADAPTER};
use crate::config::Layout;
use crate::init::adapter_uri::{AdapterUri, ensure_adapter_dir};

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
    let cache_dir = Layout::new(project_dir).cache_dir();
    let target = cache_dir.join(&source.adapter_name);
    refresh_cached_adapter(&source.source_dir, &target)?;
    cache_sibling_default_adapter(&source.source_dir, &cache_dir)?;
    write_cache_meta(project_dir, &source.adapter_value, now)?;

    Ok(source.adapter_value)
}

fn cache_sibling_default_adapter(source_dir: &Path, cache_dir: &Path) -> Result<(), Error> {
    if source_dir.file_name().and_then(|name| name.to_str()) == Some(DEFAULT_CODEX_ADAPTER) {
        return Ok(());
    }

    let Some(parent) = source_dir.parent() else {
        return Ok(());
    };
    let default_source = parent.join(DEFAULT_CODEX_ADAPTER);
    if !default_source.is_dir() {
        return Ok(());
    }

    ensure_adapter_dir(&default_source, DEFAULT_CODEX_ADAPTER)?;
    refresh_cached_adapter(&default_source, &cache_dir.join(DEFAULT_CODEX_ADAPTER))
}

fn refresh_cached_adapter(source: &Path, target: &Path) -> Result<(), Error> {
    if target.exists() {
        fs::remove_dir_all(target)?;
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
