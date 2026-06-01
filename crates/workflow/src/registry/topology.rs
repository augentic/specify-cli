//! `.specify/topology.lock` — the committed projection of each member
//! project's `project.yaml` topology facets (RFC-36).
//!
//! `registry.yaml` carries membership + location only. Project-describing
//! facets (`adapter`, `description`, `capabilities`, `keywords`) are
//! authored in each project's `.specify/project.yaml`; `specrun workspace
//! sync` resolves them into this committed lockfile so hub plan-time
//! topology (`hub_topology`) reads a single derived source offline. The
//! lockfile is machine-written (write-if-changed, mirroring
//! `.specify/context.lock`); operators never hand-edit it.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};
use specify_error::Error;
use specify_model::atomic::yaml_write;

use crate::adapter::TargetAdapter;
use crate::config::ProjectConfig;
use crate::init::adapter_name_from_value;

/// Current `topology.lock` schema version (RFC-36 shape).
pub const CURRENT_TOPOLOGY_LOCK_VERSION: u64 = 1;

/// In-memory representation of `.specify/topology.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyLock {
    /// Schema version. `1` is the only accepted value for this release.
    pub version: u64,
    /// One entry per registry member project, in registry order.
    #[serde(default)]
    pub projects: Vec<TopologyProject>,
}

/// One resolved member project — the projection of its `project.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyProject {
    /// Registry slot name — the `plan.yaml.slices[].project` binding
    /// key. Identity stays the registry name, not `project.yaml.name`.
    pub name: String,
    /// Target adapter in `name@vN` form, resolved from the project's
    /// `project.yaml.adapter`.
    pub target: String,
    /// Single-sentence domain characterisation from the project's
    /// `project.yaml`. Absent stays off the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Capability tags authored in the project's `project.yaml`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Free-form keyword tags authored in the project's `project.yaml`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Version {
    version: u64,
}

impl TopologyLock {
    /// Wrap a resolved project list into a versioned lock document.
    #[must_use]
    pub const fn from_projects(projects: Vec<TopologyProject>) -> Self {
        Self {
            version: CURRENT_TOPOLOGY_LOCK_VERSION,
            projects,
        }
    }

    /// Load + version-gate the committed cache. A missing file yields
    /// `Ok(None)` — the registry layer decides whether absence is fatal
    /// (hub plan-time topology raises `topology-cache-missing`).
    ///
    /// # Errors
    ///
    /// - [`Error::Validation`] `topology-lock-malformed` when the YAML
    ///   does not parse or carries an unsupported version.
    /// - [`Error::Validation`] `topology-lock-version-too-new` when the
    ///   version is newer than this binary supports.
    pub fn load(path: &Path) -> Result<Option<Self>, Error> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(Error::Io(err)),
        };

        let version: Version = serde_saphyr::from_str(&contents).map_err(|err| {
            malformed(format!("topology-lock-malformed: failed to read lock version: {err}"))
        })?;
        if version.version > CURRENT_TOPOLOGY_LOCK_VERSION {
            return Err(Error::validation_failed(
                "topology-lock-version-too-new",
                ".specify/topology.lock must be a supported version",
                format!(
                    "topology-lock-version-too-new: lock version {} > supported \
                     {CURRENT_TOPOLOGY_LOCK_VERSION}",
                    version.version
                ),
            ));
        }
        if version.version != CURRENT_TOPOLOGY_LOCK_VERSION {
            return Err(malformed(format!(
                "topology-lock-malformed: unsupported lock version {}; expected \
                 {CURRENT_TOPOLOGY_LOCK_VERSION}",
                version.version
            )));
        }

        let lock: Self = serde_saphyr::from_str(&contents)
            .map_err(|err| malformed(format!("topology-lock-malformed: {err}")))?;
        crate::schema::validate_topology_lock(&lock)?;
        Ok(Some(lock))
    }

    /// Atomically write the cache, after schema validation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the lock fails its schema, or
    /// the underlying I/O error on a failed write.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        crate::schema::validate_topology_lock(self)?;
        yaml_write(path, self)
    }
}

impl TopologyProject {
    /// Project one materialised slot's [`ProjectConfig`] into a resolved
    /// topology entry. `registry_name` is the slot/registry name (the
    /// binding key); `slot_dir` is the slot's project directory, used to
    /// resolve the adapter to its canonical `name@vN` ref.
    ///
    /// # Errors
    ///
    /// - [`Error::Validation`] `topology-cache-project-adapter-missing`
    ///   when the slot `project.yaml` omits `adapter`.
    /// - Any error from [`TargetAdapter::resolve`] when the adapter
    ///   cannot be resolved against the slot.
    pub fn resolve(
        registry_name: &str, config: &ProjectConfig, slot_dir: &Path,
    ) -> Result<Self, Error> {
        let adapter_value = config.adapter.as_deref().ok_or_else(|| {
            Error::validation_failed(
                "topology-cache-project-adapter-missing",
                "workspace slot project.yaml provides a target adapter",
                format!("workspace slot `{registry_name}` project.yaml omits the `adapter` field"),
            )
        })?;
        let resolved = TargetAdapter::resolve(adapter_name_from_value(adapter_value), slot_dir)?;
        let target = format!("{}@v{}", resolved.manifest.name, resolved.manifest.version);
        Ok(Self {
            name: registry_name.to_string(),
            target,
            description: config.description.clone(),
            capabilities: config.capabilities.clone(),
            keywords: config.keywords.clone(),
        })
    }
}

fn malformed(detail: String) -> Error {
    Error::validation_failed(
        "topology-lock-malformed",
        ".specify/topology.lock must be a supported topology lock file",
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_yaml_with_empties_elided() {
        let lock = TopologyLock::from_projects(vec![
            TopologyProject {
                name: "identity-contracts".to_string(),
                target: "contracts@v1".to_string(),
                description: Some("Contracts crate.".to_string()),
                capabilities: vec!["contracts".to_string()],
                keywords: Vec::new(),
            },
            TopologyProject {
                name: "identity-service".to_string(),
                target: "omnia@v1".to_string(),
                description: None,
                capabilities: Vec::new(),
                keywords: Vec::new(),
            },
        ]);

        let yaml = serde_saphyr::to_string(&lock).expect("serialize lock");
        assert!(yaml.contains("name: identity-contracts"), "{yaml}");
        assert!(!yaml.contains("keywords:"), "empty keywords elided: {yaml}");

        let parsed: TopologyLock = serde_saphyr::from_str(&yaml).expect("round-trip");
        assert_eq!(parsed, lock);
    }

    #[test]
    fn save_then_load_is_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("topology.lock");
        let lock = TopologyLock::from_projects(vec![TopologyProject {
            name: "svc".to_string(),
            target: "omnia@v1".to_string(),
            description: None,
            capabilities: Vec::new(),
            keywords: Vec::new(),
        }]);

        lock.save(&path).expect("save");
        let loaded = TopologyLock::load(&path).expect("load").expect("present");
        assert_eq!(loaded, lock);
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("absent.lock");
        assert_eq!(TopologyLock::load(&path).expect("load"), None);
    }

    #[test]
    fn version_too_new_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("topology.lock");
        fs::write(&path, "version: 99\nprojects: []\n").expect("write");
        let err = TopologyLock::load(&path).expect_err("too new");
        assert!(format!("{err:?}").contains("topology-lock-version-too-new"), "{err:?}");
    }
}
