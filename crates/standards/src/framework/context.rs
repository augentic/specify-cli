use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::framework::error::ToolingError;

/// Type-erased per-scan memo store: a memo key to its cached value.
type ScanCache = HashMap<&'static str, Arc<dyn Any + Send + Sync>>;

/// Shared scan context: framework root plus a generic per-scan memo so
/// a check that re-derives the same expensive scan (e.g. the skill-file
/// walk shared across the five SKILL.md predicates) computes it once
/// per [`Context`]. JSON Schema validators are cached process-wide by
/// [`specify_schema::cached_validator`], not here.
pub struct Context {
    framework_root: PathBuf,
    scan_cache: Mutex<ScanCache>,
}

impl Context {
    /// Construct context for the specify-cli workspace (Rust-quality checks only).
    pub fn from_specify_cli_root(root: impl AsRef<Path>) -> Result<Self, ToolingError> {
        let root = root.as_ref();
        if !is_specify_cli_root(root) {
            return Err(ToolingError::Infrastructure(format!(
                "not a specify-cli root: {}",
                root.display()
            )));
        }
        Ok(Self {
            framework_root: root.canonicalize().map_err(|source| {
                ToolingError::Infrastructure(format!("canonicalize path: {source}"))
            })?,
            scan_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Construct context when the framework root is already known (tests).
    pub fn from_framework_root(framework_root: impl AsRef<Path>) -> Result<Self, ToolingError> {
        let framework_root = framework_root.as_ref();
        if !is_framework_root(framework_root) {
            return Err(ToolingError::Infrastructure(format!(
                "not a framework root: {}",
                framework_root.display()
            )));
        }
        Ok(Self {
            framework_root: framework_root.canonicalize().map_err(|source| {
                ToolingError::Infrastructure(format!("canonicalize path: {source}"))
            })?,
            scan_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Framework repo root.
    pub fn framework_root(&self) -> &Path {
        &self.framework_root
    }

    /// `plugins/` under the framework root.
    pub fn plugins_dir(&self) -> PathBuf {
        self.framework_root.join("plugins")
    }

    /// `adapters/sources/` under the framework root.
    pub fn sources_dir(&self) -> PathBuf {
        self.framework_root.join("adapters").join("sources")
    }

    /// `adapters/targets/` under the framework root.
    pub fn targets_dir(&self) -> PathBuf {
        self.framework_root.join("adapters").join("targets")
    }

    /// `adapters/shared/` under the framework root.
    pub fn adapters_shared_dir(&self) -> PathBuf {
        self.framework_root.join("adapters").join("shared")
    }

    /// Editor-facing schema aliases under `.cursor/schemas/`.
    pub fn cursor_schema_dir(&self) -> PathBuf {
        self.framework_root.join(".cursor").join("schemas")
    }

    /// CLI-owned JSON Schemas from the local `specify-cli` checkout.
    pub fn specify_cli_schemas_dir(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("schemas")
    }

    /// Memoise an expensive, deterministic scan under `key` for this
    /// [`Context`]'s lifetime, returning a shared handle.
    ///
    /// The first call runs `build`; later calls for the same `key`
    /// return the cached value without re-running it. Only successful
    /// builds are cached — an error is propagated and the next call
    /// retries. Used by the SKILL.md predicates so the skill-file walk
    /// is performed once per [`Context`] and shared across the five
    /// frontmatter checks rather than re-walked per check.
    pub fn memoize<T, F>(&self, key: &'static str, build: F) -> Result<Arc<T>, ToolingError>
    where
        T: Send + Sync + 'static,
        F: FnOnce() -> Result<T, ToolingError>,
    {
        if let Some(cached) = self.lock_scan_cache()?.get(key) {
            return Ok(Arc::clone(cached)
                .downcast::<T>()
                .expect("scan-cache value type matches its key"));
        }
        let built = Arc::new(build()?);
        self.lock_scan_cache()?.insert(key, Arc::clone(&built) as Arc<dyn Any + Send + Sync>);
        Ok(built)
    }

    fn lock_scan_cache(&self) -> Result<std::sync::MutexGuard<'_, ScanCache>, ToolingError> {
        self.scan_cache
            .lock()
            .map_err(|_| ToolingError::Infrastructure("scan cache poisoned".into()))
    }
}

fn is_framework_root(path: &Path) -> bool {
    path.join("plugins").is_dir() && path.join("adapters").is_dir()
}

fn is_specify_cli_root(path: &Path) -> bool {
    path.join("crates/workflow").is_dir() && path.join("src/runtime").is_dir()
}
