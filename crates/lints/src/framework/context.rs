use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jsonschema::Validator;
use serde_json::Value as JsonValue;

use crate::error::ToolingError;

/// Shared scan context: framework root and schema cache.
pub struct Context {
    framework_root: PathBuf,
    schema_cache: Mutex<HashMap<PathBuf, Arc<Validator>>>,
}

impl Context {
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
            schema_cache: Mutex::new(HashMap::new()),
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

    /// Authoring schemas distributed with `specdev`.
    pub fn framework_schema_dir(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas")
    }

    /// Editor-facing schema aliases under `.cursor/schemas/`.
    pub fn cursor_schema_dir(&self) -> PathBuf {
        self.framework_root.join(".cursor").join("schemas")
    }

    /// Runtime JSON Schemas from the local `specify-cli` checkout.
    pub fn specify_cli_schemas_dir(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("schemas")
    }

    /// Lazily compile and cache a JSON Schema loaded from `path`.
    pub fn schema(&self, path: impl AsRef<Path>) -> Result<Arc<Validator>, ToolingError> {
        let path = path.as_ref().to_path_buf();
        let mut cache = self.lock_cache()?;
        if let Some(schema) = cache.get(&path) {
            return Ok(Arc::clone(schema));
        }
        let contents = std::fs::read_to_string(&path).map_err(|source| {
            ToolingError::Infrastructure(format!("read schema {}: {source}", path.display()))
        })?;
        let compiled = compile(&contents, &path)?;
        cache.insert(path, Arc::clone(&compiled));
        Ok(compiled)
    }

    /// Lazily compile and cache a JSON Schema from an in-memory `source`,
    /// keyed under the synthetic `key` so the cache stays uniform with
    /// filesystem-backed schemas.
    pub fn schema_from_source(
        &self, key: PathBuf, source: &str,
    ) -> Result<Arc<Validator>, ToolingError> {
        let mut cache = self.lock_cache()?;
        if let Some(schema) = cache.get(&key) {
            return Ok(Arc::clone(schema));
        }
        let compiled = compile(source, &key)?;
        cache.insert(key, Arc::clone(&compiled));
        Ok(compiled)
    }

    fn lock_cache(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<PathBuf, Arc<Validator>>>, ToolingError> {
        self.schema_cache
            .lock()
            .map_err(|_| ToolingError::Infrastructure("schema cache poisoned".into()))
    }
}

fn compile(source: &str, key: &Path) -> Result<Arc<Validator>, ToolingError> {
    let value: JsonValue = serde_json::from_str(source).map_err(|err| {
        ToolingError::Infrastructure(format!("parse schema {}: {err}", key.display()))
    })?;
    let compiled = jsonschema::validator_for(&value).map_err(|error| {
        ToolingError::Infrastructure(format!("compile schema {}: {error}", key.display()))
    })?;
    Ok(Arc::new(compiled))
}

fn is_framework_root(path: &Path) -> bool {
    path.join("plugins").is_dir() && path.join("adapters").is_dir()
}
