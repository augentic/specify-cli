//! Operator-curated component catalog (component catalog contract).
//!
//! The catalog lives at `.specify/design-system/components.yaml` and
//! declares shared UI components that the Vectis target factors into
//! shared code at build time. The file is opt-in — projects without it
//! work exactly as before.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::{Error, Result};

use crate::schema;

/// On-disk path relative to project root.
const CATALOG_REL: &str = ".specify/design-system/components.yaml";

/// Closed status enum for catalog entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentStatus {
    /// The build should factor this as a shared component.
    Confirmed,
    /// The operator has decided this is not a real shared component;
    /// suppresses `slice-catalog-drift` warnings.
    Rejected,
}

/// A single component catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentEntry {
    /// Whether the component is confirmed for shared factoring or
    /// rejected (suppresses drift warnings).
    pub status: ComponentStatus,
    /// Human-readable note for operators and agents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The operator-curated component catalog.
///
/// Validated against `schemas/design-system/components.schema.json` on
/// load. Absent catalogs are represented as `None` at the call site —
/// this struct always represents a successfully loaded and validated
/// catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentsCatalog {
    /// Schema version (currently pinned to `1`).
    pub version: u32,
    /// Map of kebab-case component slugs to their metadata.
    pub components: BTreeMap<String, ComponentEntry>,
}

impl ComponentsCatalog {
    /// Load and validate the catalog from a project root.
    ///
    /// Returns `Ok(None)` when the catalog file does not exist (opt-in).
    /// Returns `Err` when the file exists but fails YAML parse or schema
    /// validation.
    ///
    /// # Errors
    ///
    /// - [`Error::Filesystem`] if the file exists but cannot be read.
    /// - [`Error::Validation`] if the file fails schema validation.
    pub fn load(project_dir: &Path) -> Result<Option<Self>> {
        let path = project_dir.join(CATALOG_REL);
        if !path.is_file() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        Self::from_yaml(&content, &path).map(Some)
    }

    /// Parse and validate catalog YAML content.
    ///
    /// `source_path` is used only for error messages.
    ///
    /// # Errors
    ///
    /// - [`Error::Validation`] if YAML parsing or schema validation fails.
    pub fn from_yaml(content: &str, source_path: &Path) -> Result<Self> {
        schema::validate_components_yaml(content, source_path)?;
        let catalog: Self = serde_saphyr::from_str(content).map_err(|err| {
            Error::validation_failed(
                "catalog-schema",
                "components.yaml conforms to schemas/design-system/components.schema.json",
                format!("{}: deserialise failed: {err}", source_path.display()),
            )
        })?;
        Ok(catalog)
    }

    /// Return the path where the catalog lives relative to a project root.
    #[must_use]
    pub fn path_in(project_dir: &Path) -> PathBuf {
        project_dir.join(CATALOG_REL)
    }

    /// Slugs whose status is `confirmed`.
    #[must_use]
    pub fn confirmed_slugs(&self) -> Vec<&str> {
        self.components
            .iter()
            .filter(|(_, entry)| entry.status == ComponentStatus::Confirmed)
            .map(|(slug, _)| slug.as_str())
            .collect()
    }

    /// Slugs whose status is `rejected`.
    #[must_use]
    pub fn rejected_slugs(&self) -> Vec<&str> {
        self.components
            .iter()
            .filter(|(_, entry)| entry.status == ComponentStatus::Rejected)
            .map(|(slug, _)| slug.as_str())
            .collect()
    }

    /// Look up the status of a component by slug.
    #[must_use]
    pub fn status_of(&self, slug: &str) -> Option<ComponentStatus> {
        self.components.get(slug).map(|entry| entry.status)
    }
}

#[cfg(test)]
mod tests;
