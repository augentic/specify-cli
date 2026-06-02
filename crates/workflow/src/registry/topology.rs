//! `.specify/topology.lock` — the committed projection of each member
//! project's identity (RFC-36).
//!
//! `registry.yaml` carries membership + location only. A project's
//! authored intent (`adapter`, `description`) lives in its
//! `.specify/project.yaml`; its derived identity — the `surface[]` of
//! owned units and a `recent[]` tail of merge outcomes — is a
//! deterministic structural projection of its baseline
//! (`.specify/specs/` + `.specify/journal.jsonl`). `specrun workspace
//! sync` resolves both into this committed lockfile so hub plan-time
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

/// One resolved member project — its authored intent plus the
/// deterministic projection of its baseline (RFC-36).
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
    /// Deterministic baseline surface: one entry per
    /// `.specify/specs/<unit>/spec.md`, projected from the slot's
    /// merged baseline. Empty stays off the wire (greenfield).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface: Vec<Surface>,
    /// The last `M` `slice.archive.created` outcome summaries from the
    /// slot's journal ledger, in append order. Empty stays off the wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<String>,
    /// Accepted Decision Records projected from `.specify/decisions/`
    /// (RFC-37), the most recent `K` in `DEC-NNNN` ascending order.
    /// The third routing-identity axis — *why* the project is shaped the
    /// way it is. Empty stays off the wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    /// Count of accepted decisions elided past the `K` cap. Absent when
    /// the catalogue fits within `K`.
    #[serde(default, rename = "decisions-more", skip_serializing_if = "Option::is_none")]
    pub decisions_more: Option<u64>,
}

/// One accepted Decision Record projected into routing identity.
///
/// RFC-37 §"Decision Records as an identity source". Title only — no body,
/// `Context`, or `Consequences` prose is projected. Shared by
/// [`TopologyProject`] and the reconciliation envelope's `ProjectRef`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// The durable `DEC-NNNN` id.
    pub id: String,
    /// The record's H1 heading text.
    pub title: String,
}

/// One baseline unit's projected surface (RFC-36 §"Projection contract").
///
/// The unit slug and a bounded sample of its requirement titles. Shared
/// by [`TopologyProject`] and the reconciliation envelope's `ProjectRef`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    /// Unit directory slug under `.specify/specs/`.
    pub unit: String,
    /// Requirement-block headings (`Requirement.name`, inline tag
    /// stripped) in `REQ-NNN` id order, capped at
    /// [`super::identity::SURFACE_TITLE_CAP`].
    pub requirements: Vec<String>,
    /// Count of requirement titles elided past the cap. Absent when
    /// the unit fits within the cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<u64>,
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
    /// Project one materialised slot into a resolved topology entry:
    /// the slot [`ProjectConfig`]'s authored intent (`description`) and
    /// resolved `target`, plus the deterministic baseline projection
    /// (`surface[]` / `recent[]`) read from `slot_dir` (RFC-36).
    /// `registry_name` is the slot/registry name (the binding key);
    /// `slot_dir` is the slot's project directory, used both to resolve
    /// the adapter to its canonical `name@vN` ref and as the baseline
    /// projection root.
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
        let projection = super::identity::project_baseline(slot_dir)?;
        Ok(Self {
            name: registry_name.to_string(),
            target,
            description: config.description.clone(),
            surface: projection.surface,
            recent: projection.recent,
            decisions: projection.decisions,
            decisions_more: projection.decisions_more,
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
                surface: vec![Surface {
                    unit: "identity-api".to_string(),
                    requirements: vec!["Authenticate user".to_string()],
                    more: None,
                }],
                recent: Vec::new(),
                decisions: Vec::new(),
                decisions_more: None,
            },
            TopologyProject {
                name: "identity-service".to_string(),
                target: "omnia@v1".to_string(),
                description: None,
                surface: Vec::new(),
                recent: Vec::new(),
                decisions: Vec::new(),
                decisions_more: None,
            },
        ]);

        let yaml = serde_saphyr::to_string(&lock).expect("serialize lock");
        assert!(yaml.contains("name: identity-contracts"), "{yaml}");
        assert!(!yaml.contains("recent:"), "empty recent elided: {yaml}");

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
            surface: Vec::new(),
            recent: Vec::new(),
            decisions: Vec::new(),
            decisions_more: None,
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
