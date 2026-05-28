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
mod tests {
    use super::*;

    #[test]
    fn load_returns_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = ComponentsCatalog::load(dir.path()).expect("no error");
        assert!(result.is_none());
    }

    #[test]
    fn confirmed_slugs_filters_correctly() {
        let catalog = ComponentsCatalog {
            version: 1,
            components: BTreeMap::from([
                (
                    "tab-bar".to_string(),
                    ComponentEntry {
                        status: ComponentStatus::Confirmed,
                        description: None,
                    },
                ),
                (
                    "card-row".to_string(),
                    ComponentEntry {
                        status: ComponentStatus::Confirmed,
                        description: None,
                    },
                ),
                (
                    "hero-banner".to_string(),
                    ComponentEntry {
                        status: ComponentStatus::Rejected,
                        description: None,
                    },
                ),
            ]),
        };
        let mut slugs = catalog.confirmed_slugs();
        slugs.sort_unstable();
        assert_eq!(slugs, vec!["card-row", "tab-bar"]);
    }

    #[test]
    fn rejected_slugs_filters_correctly() {
        let catalog = ComponentsCatalog {
            version: 1,
            components: BTreeMap::from([
                (
                    "tab-bar".to_string(),
                    ComponentEntry {
                        status: ComponentStatus::Confirmed,
                        description: None,
                    },
                ),
                (
                    "hero-banner".to_string(),
                    ComponentEntry {
                        status: ComponentStatus::Rejected,
                        description: None,
                    },
                ),
            ]),
        };
        assert_eq!(catalog.rejected_slugs(), vec!["hero-banner"]);
    }

    #[test]
    fn status_of_returns_correct_variant() {
        let catalog = ComponentsCatalog {
            version: 1,
            components: BTreeMap::from([(
                "tab-bar".to_string(),
                ComponentEntry {
                    status: ComponentStatus::Confirmed,
                    description: Some("Bottom nav".to_string()),
                },
            )]),
        };
        assert_eq!(catalog.status_of("tab-bar"), Some(ComponentStatus::Confirmed));
        assert_eq!(catalog.status_of("missing"), None);
    }

    #[test]
    fn round_trip_yaml() {
        let yaml = "version: 1\ncomponents:\n  tab-bar:\n    status: confirmed\n    description: \"Bottom navigation\"\n  hero-banner:\n    status: rejected\n";
        let path = Path::new("test.yaml");
        let catalog = ComponentsCatalog::from_yaml(yaml, path).expect("valid");
        assert_eq!(catalog.version, 1);
        assert_eq!(catalog.components.len(), 2);
        assert_eq!(catalog.status_of("tab-bar"), Some(ComponentStatus::Confirmed));
        assert_eq!(catalog.status_of("hero-banner"), Some(ComponentStatus::Rejected));
    }

    #[test]
    fn rejects_missing_version() {
        let yaml = "components:\n  tab-bar:\n    status: confirmed\n";
        let path = Path::new("test.yaml");
        ComponentsCatalog::from_yaml(yaml, path).unwrap_err();
    }

    #[test]
    fn rejects_invalid_status() {
        let yaml = "version: 1\ncomponents:\n  tab-bar:\n    status: pending\n";
        let path = Path::new("test.yaml");
        ComponentsCatalog::from_yaml(yaml, path).unwrap_err();
    }

    #[test]
    fn rejects_non_kebab_slug() {
        let yaml = "version: 1\ncomponents:\n  TabBar:\n    status: confirmed\n";
        let path = Path::new("test.yaml");
        ComponentsCatalog::from_yaml(yaml, path).unwrap_err();
    }

    #[test]
    fn empty_components_is_valid() {
        let yaml = "version: 1\ncomponents: {}\n";
        let path = Path::new("test.yaml");
        let catalog = ComponentsCatalog::from_yaml(yaml, path).expect("valid");
        assert!(catalog.components.is_empty());
        assert!(catalog.confirmed_slugs().is_empty());
    }
}
