//! `ProjectConfig` — the in-memory model of `.specify/project.yaml` plus
//! the family of path helpers every subcommand reaches for when it needs
//! to locate `.specify/changes/`, `.specify/specs/`, `.specify/.cache/`,
//! or `.specify/archive/`.
//!
//! `ProjectConfig::load` is the single choke point for the
//! `specify_version` floor check: any subcommand that parses
//! `project.yaml` picks up the check automatically. See
//! `DECISIONS.md` ("Change I — CLI exit codes and version-floor
//! semantics") for the surrounding context.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;
use specify_federation::FederationConfig;

/// In-memory representation of `.specify/project.yaml`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ProjectConfig {
    /// Project name (defaults to the project directory name at init time).
    pub name: String,

    /// Free-text description of the project's tech stack, architecture,
    /// and testing approach. Falls back to `schema.domain` when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,

    /// Schema identifier — either a bare name (`omnia`) or a URL.
    pub schema: String,

    /// Minimum `specify` CLI version required to operate on this project.
    /// Written by `specify init` as the running binary's version and
    /// enforced by [`ProjectConfig::load`] via the `semver` crate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specify_version: Option<String>,

    /// Map of brief id (e.g. `proposal`, `specs`, `design`, `tasks`) to a
    /// path (relative to `.specify/`) of a markdown file containing extra
    /// rules for that brief. Scaffolded with one empty entry per
    /// `pipeline.define` brief by `specify init`.
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
}

impl ProjectConfig {
    /// Load `.specify/project.yaml` from `project_dir`.
    ///
    /// - Returns `Err(Error::NotInitialized)` if the file is absent.
    /// - Propagates YAML parse failures as `Error::Yaml`.
    /// - Enforces the `specify_version` floor: if the pinned version in
    ///   the file is newer than `CARGO_PKG_VERSION`, returns
    ///   `Err(Error::SpecifyVersionTooOld { required, found })`.
    ///   Unparseable pinned versions are tolerated — we prefer a
    ///   permissive stance for a human-edited file.
    pub fn load(project_dir: &Path) -> Result<Self, Error> {
        let path = Self::config_path(project_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotInitialized);
            }
            Err(err) => return Err(Error::Io(err)),
        };
        let cfg: ProjectConfig = serde_yaml::from_str(&text)?;

        let current = env!("CARGO_PKG_VERSION");
        if let Some(required) = &cfg.specify_version
            && version_is_older(current, required)
        {
            return Err(Error::SpecifyVersionTooOld {
                required: required.clone(),
                found: current.to_string(),
            });
        }

        Ok(cfg)
    }

    /// Absolute path to `<project_dir>/.specify/project.yaml`.
    pub fn config_path(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join("project.yaml")
    }

    /// Absolute path to `<project_dir>/.specify/`.
    pub fn specify_dir(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify")
    }

    /// Absolute path to `<project_dir>/.specify/changes/`.
    pub fn changes_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join("changes")
    }

    /// Absolute path to `<project_dir>/.specify/specs/`.
    pub fn specs_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join("specs")
    }

    /// Absolute path to `<project_dir>/.specify/.cache/`.
    pub fn cache_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join(".cache")
    }

    /// Absolute path to `<project_dir>/.specify/archive/`. Not listed in
    /// RFC-1 §`config.rs` but needed by the merge engine; centralised
    /// here so there is still exactly one place the convention lives.
    pub fn archive_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join("archive")
    }

    /// Resolve a `rules` value to an absolute path under `.specify/`.
    /// Returns `None` when the brief has no override (absent or empty).
    pub fn rule_path(&self, project_dir: &Path, brief_id: &str) -> Option<PathBuf> {
        let value = self.rules.get(brief_id)?;
        if value.is_empty() {
            return None;
        }
        Some(Self::specify_dir(project_dir).join(value))
    }
}

impl FederationConfig for ProjectConfig {}

/// Returns `true` when `current < required` under semver ordering.
/// Unparseable versions are treated as "not older" — we don't want a
/// typo in a human-edited `project.yaml` to brick the project.
fn version_is_older(current: &str, required: &str) -> bool {
    let (Ok(cur), Ok(req)) = (semver::Version::parse(current), semver::Version::parse(required))
    else {
        return false;
    };
    cur < req
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_config(dir: &Path, yaml: &str) {
        let specify = dir.join(".specify");
        fs::create_dir_all(&specify).expect("create .specify");
        fs::write(specify.join("project.yaml"), yaml).expect("write project.yaml");
    }

    #[test]
    fn path_helpers_return_expected_subpaths() {
        let base = Path::new("/a/b");
        assert_eq!(ProjectConfig::specify_dir(base), PathBuf::from("/a/b/.specify"));
        assert_eq!(ProjectConfig::config_path(base), PathBuf::from("/a/b/.specify/project.yaml"));
        assert_eq!(ProjectConfig::changes_dir(base), PathBuf::from("/a/b/.specify/changes"));
        assert_eq!(ProjectConfig::specs_dir(base), PathBuf::from("/a/b/.specify/specs"));
        assert_eq!(ProjectConfig::cache_dir(base), PathBuf::from("/a/b/.specify/.cache"));
        assert_eq!(ProjectConfig::archive_dir(base), PathBuf::from("/a/b/.specify/archive"));
    }

    fn sample_cfg(rules: BTreeMap<String, String>) -> ProjectConfig {
        ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            schema: "omnia".to_string(),
            specify_version: None,
            rules,
        }
    }

    #[test]
    fn rule_path_empty_map_is_none() {
        let cfg = sample_cfg(BTreeMap::new());
        assert!(cfg.rule_path(Path::new("/proj"), "proposal").is_none());
    }

    #[test]
    fn rule_path_empty_value_is_none() {
        let mut rules = BTreeMap::new();
        rules.insert("proposal".to_string(), String::new());
        let cfg = sample_cfg(rules);
        assert!(cfg.rule_path(Path::new("/proj"), "proposal").is_none());
    }

    #[test]
    fn rule_path_resolves_under_specify_dir() {
        let mut rules = BTreeMap::new();
        rules.insert("proposal".to_string(), "rules/proposal.md".to_string());
        let cfg = sample_cfg(rules);
        assert_eq!(
            cfg.rule_path(Path::new("/proj"), "proposal"),
            Some(PathBuf::from("/proj/.specify/rules/proposal.md"))
        );
    }

    #[test]
    fn load_returns_not_initialized_when_missing() {
        let tmp = tempdir().unwrap();
        let err = ProjectConfig::load(tmp.path()).expect_err("missing file errs");
        assert!(matches!(err, Error::NotInitialized));
    }

    #[test]
    fn load_refuses_future_specify_version() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\nschema: omnia\nspecify_version: \"99.0.0\"\n");
        let err = ProjectConfig::load(tmp.path()).expect_err("future version rejected");
        match err {
            Error::SpecifyVersionTooOld { required, found } => {
                assert_eq!(required, "99.0.0");
                assert_eq!(found, env!("CARGO_PKG_VERSION"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn load_permits_equal_or_older_specify_version() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\nschema: omnia\nspecify_version: \"0.0.1\"\n");
        ProjectConfig::load(tmp.path()).expect("older version loads");

        let tmp = tempdir().unwrap();
        let exact = env!("CARGO_PKG_VERSION");
        write_config(
            tmp.path(),
            &format!("name: demo\nschema: omnia\nspecify_version: \"{exact}\"\n"),
        );
        ProjectConfig::load(tmp.path()).expect("exact version loads");
    }

    #[test]
    fn load_ignores_unparseable_pinned_version() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\nschema: omnia\nspecify_version: not-a-semver\n");
        let cfg = ProjectConfig::load(tmp.path()).expect("unparseable version is permissive");
        assert_eq!(cfg.specify_version.as_deref(), Some("not-a-semver"));
    }
}
