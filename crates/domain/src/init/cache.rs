//! Capability cache management: copy the resolved source into
//! `.specify/.cache/<capability>/`, mirror the bundled `codex` sibling
//! when present, and stamp `cache_meta.yaml` with the resolved URI.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use crate::capability::{CacheMeta, DEFAULT_CODEX_CAPABILITY};
use crate::config::LayoutExt;
use specify_error::Error;

use crate::init::capability_uri::{CapabilityUri, ensure_capability_dir};

#[derive(Debug)]
pub(crate) struct CachedCapability {
    pub(crate) capability_value: String,
}

pub(crate) fn cache_capability(
    capability: &str, project_dir: &Path, now: DateTime<Utc>,
) -> Result<CachedCapability, Error> {
    if capability.trim().is_empty() || capability != capability.trim() {
        return Err(Error::Diag {
            code: "capability-arg-malformed",
            detail:
                "<capability> must be non-empty and must not have leading or trailing whitespace"
                    .to_string(),
        });
    }

    let source = CapabilityUri::parse(capability, project_dir)?;
    let cache_dir = project_dir.layout().cache_dir();
    let target = cache_dir.join(&source.capability_name);
    refresh_cached_capability(&source.source_dir, &target)?;
    cache_sibling_default_capability(&source.source_dir, &cache_dir)?;
    write_cache_meta(project_dir, &source.capability_value, now)?;

    Ok(CachedCapability {
        capability_value: source.capability_value,
    })
}

fn cache_sibling_default_capability(source_dir: &Path, cache_dir: &Path) -> Result<(), Error> {
    if source_dir.file_name().and_then(|name| name.to_str()) == Some(DEFAULT_CODEX_CAPABILITY) {
        return Ok(());
    }

    let Some(parent) = source_dir.parent() else {
        return Ok(());
    };
    let default_source = parent.join(DEFAULT_CODEX_CAPABILITY);
    if !default_source.is_dir() {
        return Ok(());
    }

    ensure_capability_dir(&default_source, DEFAULT_CODEX_CAPABILITY)?;
    refresh_cached_capability(&default_source, &cache_dir.join(DEFAULT_CODEX_CAPABILITY))
}

fn refresh_cached_capability(source: &Path, target: &Path) -> Result<(), Error> {
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

fn write_cache_meta(
    project_dir: &Path, capability_value: &str, now: DateTime<Utc>,
) -> Result<(), Error> {
    let meta = CacheMeta {
        schema_url: capability_value.to_string(),
        fetched_at: now.to_rfc3339(),
    };
    let meta_path = CacheMeta::path(project_dir);
    let serialised = serde_saphyr::to_string(&meta)?;
    fs::write(meta_path, serialised)?;
    Ok(())
}
