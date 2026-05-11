//! Cache metadata: the on-disk `.specify/.cache/.cache-meta.yaml` file.
//!
//! The agent owns writes to this file; the CLI only reads it while
//! deciding whether the cache on disk matches the `schema` value in
//! `.specify/project.yaml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::capability::ValidationResult;
use crate::capability::capability::validate_against_schema;

const CACHE_META_JSON_SCHEMA: &str = include_str!("../../../../schemas/cache-meta.schema.json");

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

    /// Load `.cache-meta.yaml`:
    /// - `Ok(None)` if the file is missing (cache empty).
    /// - `Ok(Some(meta))` on a successful parse.
    /// - `Err(Error::Diag { code: "cache-meta-malformed" | "cache-meta-read-failed", .. })`
    ///   if the file exists but cannot be parsed.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(project_dir: &Path) -> Result<Option<Self>, Error> {
        let path = Self::path(project_dir);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(Error::Diag {
                    code: "cache-meta-read-failed",
                    detail: format!("failed to read {}: {err}", path.display()),
                });
            }
        };
        let meta: Self = serde_saphyr::from_str(&contents).map_err(|err| Error::Diag {
            code: "cache-meta-malformed",
            detail: format!("invalid cache-meta at {}: {err}", path.display()),
        })?;
        Ok(Some(meta))
    }

    /// Validate this `CacheMeta` against the embedded
    /// `schemas/cache-meta.schema.json`.
    #[must_use]
    pub fn validate_structure(&self) -> Vec<ValidationResult> {
        let value: serde_json::Value = match serde_json::to_value(self) {
            Ok(v) => v,
            Err(err) => {
                return vec![ValidationResult::Fail {
                    rule_id: "cache-meta.serializable".into(),
                    rule: "cache-meta is serializable to JSON".into(),
                    detail: err.to_string(),
                }];
            }
        };
        validate_against_schema(
            CACHE_META_JSON_SCHEMA,
            "cache-meta.valid",
            "cache-meta.yaml conforms to schemas/cache-meta.schema.json",
            &value,
        )
    }

    /// True when the cache on disk was populated from `schema_value`.
    ///
    /// Encoding:
    /// - Bare names (no `://`) → `schema_url == format!("local:{name}")`.
    /// - URL-shaped values → `schema_url == schema_value` (exact match,
    ///   including `@ref` if present).
    #[must_use]
    pub fn matches(&self, schema_value: &str) -> bool {
        if schema_value.contains("://") {
            self.schema_url == schema_value
        } else {
            self.schema_url == format!("local:{schema_value}")
        }
    }
}
