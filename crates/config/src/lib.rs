#![allow(
    clippy::multiple_crate_versions,
    reason = "ProjectConfig re-exports `specify_tool::Tool`, which transitively pulls in Wasmtime/WASI duplicate versions."
)]

//! `ProjectConfig` — the in-memory model of `.specify/project.yaml`.
//!
//! Hosts the family of path helpers every subcommand reaches for when
//! it needs to locate `.specify/slices/`, `.specify/.cache/`,
//! `.specify/archive/`, or the operator-facing platform artifacts at
//! the repo root (`registry.yaml`, `plan.yaml`, `change.md`).
//!
//! Layout boundary: `.specify/` holds framework-managed state that the
//! CLI owns (configuration, working slices, archive, cache, workspace
//! clones, plan-authoring scratch, the advisory plan lock).
//! Operator-facing platform artifacts that are PR-reviewed and durable
//! live at the repo root.
//!
//! `ProjectConfig` stays capability-agnostic. Domain-specific baseline
//! locations are owned by the active capability and synthesised at the
//! call site (currently `src/commands/slice.rs::artifact_classes`).
//!
//! `ProjectConfig::load` is the single choke point for the
//! `specify_version` floor check: any subcommand that parses
//! `project.yaml` picks up the check automatically.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// In-memory representation of `.specify/project.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProjectConfig {
    /// Project name (defaults to the project directory name at init time).
    pub name: String,

    /// Free-text description of the project's tech stack, architecture,
    /// and testing approach. Falls back to `capability.domain` when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,

    /// Capability identifier — either a bare name (`omnia`) or a URL.
    /// Absent for registry-only platform hubs (`hub: true`); see the
    /// `hub` field for the discriminator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,

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

    /// Project-scope WASI tool declarations. These are generic extension
    /// points owned by `specify-tool`, not by any capability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<specify_tool::Tool>,

    /// `true` when this project is a registry-only **platform hub**.
    /// Hubs hold platform-level state — `registry.yaml`, `change.md`,
    /// `plan.yaml`, `workspace/` — but never appear in their own
    /// `registry.yaml` and have phase pipelines disabled. Hubs **omit**
    /// the `capability:` field entirely; the absence of `capability:`
    /// together with `hub: true` is the discriminator.
    /// Defaults to `false`; serialised only when `true` so non-hub
    /// `project.yaml` files round-trip byte-stable.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hub: bool,
}

// `serde`'s `skip_serializing_if` requires `Fn(&T) -> bool`, so the
// `&bool` parameter is forced — we can't take by value.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

impl ProjectConfig {
    /// Load `.specify/project.yaml` from `project_dir`.
    ///
    /// - Returns `Err(Error::NotInitialized)` if the file is absent.
    /// - Propagates YAML parse failures as `Error::Yaml`.
    /// - Enforces the `specify_version` floor: if the pinned version in
    ///   the file is newer than `CARGO_PKG_VERSION`, returns
    ///   `Err(Error::CliTooOld { required, found })`.
    ///   Unparseable pinned versions are tolerated — we prefer a
    ///   permissive stance for a human-edited file.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(project_dir: &Path) -> Result<Self, Error> {
        let path = Self::config_path(project_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotInitialized);
            }
            Err(err) => return Err(Error::Io(err)),
        };

        let cfg: Self = serde_saphyr::from_str(&text)?;

        let current = env!("CARGO_PKG_VERSION");
        if let Some(required) = &cfg.specify_version
            && version_is_older(current, required)
        {
            return Err(Error::CliTooOld {
                required: required.clone(),
                found: current.to_string(),
            });
        }

        Ok(cfg)
    }

    /// Walk `start_dir` and its ancestors looking for the first directory
    /// that contains `.specify/project.yaml`. Returns `Ok(None)` when no
    /// ancestor is initialised.
    ///
    /// # Errors
    ///
    /// Returns an error if a filesystem probe fails (other than the
    /// "not found" case, which is expressed as `Ok(None)` per ancestor).
    pub fn find_root(start_dir: &Path) -> Result<Option<PathBuf>, Error> {
        for candidate in start_dir.ancestors() {
            let config_path = Self::config_path(candidate);
            match config_path.try_exists() {
                Ok(true) => return Ok(Some(candidate.to_path_buf())),
                Ok(false) => {}
                Err(err) => return Err(Error::Io(err)),
            }
        }
        Ok(None)
    }

    /// Absolute path to `<project_dir>/.specify/project.yaml`.
    #[must_use]
    pub fn config_path(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join("project.yaml")
    }

    /// Absolute path to `<project_dir>/.specify/`.
    #[must_use]
    pub fn specify_dir(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify")
    }

    /// Absolute path to `<project_dir>/.specify/slices/`.
    #[must_use]
    pub fn slices_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join(specify_slice::SLICES_DIR_NAME)
    }

    /// Absolute path to `<project_dir>/registry.yaml` — the platform
    /// catalogue. Platform-level artifact, lives at the repo root.
    #[must_use]
    pub fn registry_path(project_dir: &Path) -> PathBuf {
        project_dir.join("registry.yaml")
    }

    /// Absolute path to `<project_dir>/plan.yaml` — the change
    /// plan. Platform-level artifact, lives at the repo root.
    #[must_use]
    pub fn plan_path(project_dir: &Path) -> PathBuf {
        project_dir.join("plan.yaml")
    }

    /// Absolute path to `<project_dir>/change.md` — the umbrella
    /// operator brief at the repo root. Platform-level artifact.
    #[must_use]
    pub fn change_brief_path(project_dir: &Path) -> PathBuf {
        project_dir.join(specify_capability::CHANGE_BRIEF_FILENAME)
    }

    /// Absolute path to `<project_dir>/.specify/.cache/`.
    #[must_use]
    pub fn cache_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join(".cache")
    }

    /// Absolute path to `<project_dir>/.specify/archive/`. Centralised
    /// here so there is exactly one place the convention lives.
    #[must_use]
    pub fn archive_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join("archive")
    }

    /// Resolve a `rules` value to an absolute path under `.specify/`.
    /// Returns `None` when the brief has no override (absent or empty).
    #[must_use]
    pub fn rule_path(&self, project_dir: &Path, brief_id: &str) -> Option<PathBuf> {
        let value = self.rules.get(brief_id)?;
        if value.is_empty() {
            return None;
        }
        Some(Self::specify_dir(project_dir).join(value))
    }
}

/// Detect whether `project_dir` lives below `.specify/workspace/<peer>/`.
///
/// This is a path-ancestry predicate only. Context generation uses the
/// shared posture to skip init-time `AGENTS.md` creation in workspace
/// clones and to refuse standalone generation there; callers that need
/// a fully initialized clone can layer `.specify/project.yaml` or
/// plan-file guards on top.
#[must_use]
pub fn is_workspace_clone_path(project_dir: &Path) -> bool {
    let components: Vec<_> = project_dir.components().collect();
    components.windows(3).any(|w| {
        w[0].as_os_str() == ".specify"
            && w[1].as_os_str() == "workspace"
            && !w[2].as_os_str().is_empty()
    })
}

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
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_config(dir: &Path, yaml: &str) {
        let specify = dir.join(".specify");
        fs::create_dir_all(&specify).expect("create .specify");
        fs::write(specify.join("project.yaml"), yaml).expect("write project.yaml");
    }

    #[test]
    fn specify_subpaths() {
        let base = Path::new("/a/b");
        assert_eq!(ProjectConfig::specify_dir(base), PathBuf::from("/a/b/.specify"));
        assert_eq!(ProjectConfig::config_path(base), PathBuf::from("/a/b/.specify/project.yaml"));
        assert_eq!(ProjectConfig::slices_dir(base), PathBuf::from("/a/b/.specify/slices"));
        assert_eq!(ProjectConfig::registry_path(base), PathBuf::from("/a/b/registry.yaml"));
        assert_eq!(ProjectConfig::plan_path(base), PathBuf::from("/a/b/plan.yaml"));
        assert_eq!(ProjectConfig::change_brief_path(base), PathBuf::from("/a/b/change.md"));
        assert_eq!(ProjectConfig::cache_dir(base), PathBuf::from("/a/b/.specify/.cache"));
        assert_eq!(ProjectConfig::archive_dir(base), PathBuf::from("/a/b/.specify/archive"));
    }

    fn sample_cfg(rules: BTreeMap<String, String>) -> ProjectConfig {
        ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            capability: Some("omnia".to_string()),
            specify_version: None,
            rules,
            tools: Vec::new(),
            hub: false,
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
        write_config(tmp.path(), "name: demo\ncapability: omnia\nspecify_version: \"99.0.0\"\n");
        let err = ProjectConfig::load(tmp.path()).expect_err("future version rejected");
        match err {
            Error::CliTooOld { required, found } => {
                assert_eq!(required, "99.0.0");
                assert_eq!(found, env!("CARGO_PKG_VERSION"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn load_accepts_floor_lte_current() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\ncapability: omnia\nspecify_version: \"0.0.1\"\n");
        ProjectConfig::load(tmp.path()).expect("older version loads");

        let tmp = tempdir().unwrap();
        let exact = env!("CARGO_PKG_VERSION");
        write_config(
            tmp.path(),
            &format!("name: demo\ncapability: omnia\nspecify_version: \"{exact}\"\n"),
        );
        ProjectConfig::load(tmp.path()).expect("exact version loads");
    }

    #[test]
    fn load_allows_invalid_pinned_version() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\ncapability: omnia\nspecify_version: not-a-semver\n");
        let cfg = ProjectConfig::load(tmp.path()).expect("unparseable version is permissive");
        assert_eq!(cfg.specify_version.as_deref(), Some("not-a-semver"));
    }

    #[test]
    fn hub_field_defaults_false_and_round_trips_when_true() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\ncapability: omnia\n");
        let cfg = ProjectConfig::load(tmp.path()).expect("loads");
        assert!(!cfg.hub, "hub must default to false when absent");
        assert_eq!(cfg.capability.as_deref(), Some("omnia"));
        assert!(cfg.tools.is_empty(), "tools must default empty when absent");

        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\nhub: true\n");
        let cfg = ProjectConfig::load(tmp.path()).expect("loads");
        assert!(cfg.hub, "hub: true must round-trip through deserialize");
        assert!(cfg.capability.is_none(), "hub project.yaml must omit capability:");
    }

    #[test]
    fn hub_field_omitted_when_false_in_serialise() {
        let cfg = ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            capability: Some("omnia".to_string()),
            specify_version: None,
            rules: BTreeMap::new(),
            tools: Vec::new(),
            hub: false,
        };
        let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
        assert!(!yaml.contains("hub:"), "hub: false should be omitted, got:\n{yaml}");
        assert!(yaml.contains("capability: omnia"), "capability: must serialise, got:\n{yaml}");
    }

    #[test]
    fn hub_field_serialised_when_true_and_capability_omitted() {
        let cfg = ProjectConfig {
            name: "platform".to_string(),
            domain: None,
            capability: None,
            specify_version: None,
            rules: BTreeMap::new(),
            tools: Vec::new(),
            hub: true,
        };
        let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
        assert!(yaml.contains("hub: true"), "hub: true must serialise, got:\n{yaml}");
        assert!(
            !yaml.contains("capability:"),
            "hub project.yaml must omit `capability:`, got:\n{yaml}"
        );
    }

    #[test]
    fn tools_field_parses_and_serialises_when_present() {
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "name: demo\ncapability: omnia\ntools:\n  - name: contract\n    version: 1.0.0\n    source: https://example.com/contract.wasm\n",
        );
        let cfg = ProjectConfig::load(tmp.path()).expect("loads");
        assert_eq!(cfg.tools.len(), 1);
        assert_eq!(cfg.tools[0].name, "contract");
        assert!(matches!(
            &cfg.tools[0].source,
            specify_tool::ToolSource::HttpsUri(uri) if uri == "https://example.com/contract.wasm"
        ));

        let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
        assert!(yaml.contains("tools:"), "tools should serialise when present, got:\n{yaml}");
        assert!(
            yaml.contains("source: https://example.com/contract.wasm"),
            "tool source should stay in string form, got:\n{yaml}"
        );
    }

    #[test]
    fn tools_field_omitted_when_empty() {
        let cfg = sample_cfg(BTreeMap::new());
        let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
        assert!(!yaml.contains("tools:"), "empty tools should be omitted, got:\n{yaml}");
    }

    #[test]
    fn workspace_clone_path_detects_literal_workspace_slot() {
        let path = Path::new("/repo/.specify/workspace/orders");
        assert!(is_workspace_clone_path(path));
    }

    #[test]
    fn workspace_clone_path_detects_nested_directory_inside_slot() {
        let path = Path::new("/repo/.specify/workspace/orders/src/service");
        assert!(is_workspace_clone_path(path));
    }

    #[test]
    fn workspace_clone_path_rejects_non_workspace_paths() {
        assert!(!is_workspace_clone_path(Path::new("/repo")));
        assert!(!is_workspace_clone_path(Path::new("/repo/.specify")));
        assert!(!is_workspace_clone_path(Path::new("/repo/.specify/workspace")));
    }

    #[test]
    fn find_root_walks_up_to_specify_project() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let nested = root.join("sub").join("dir");
        fs::create_dir_all(&nested).expect("mkdir nested");
        write_config(root, "name: demo\ncapability: omnia\n");

        assert_eq!(ProjectConfig::find_root(root).unwrap().as_deref(), Some(root));
        assert_eq!(ProjectConfig::find_root(&nested).unwrap().as_deref(), Some(root));
    }

    #[test]
    fn find_root_returns_none_outside_initialised_tree() {
        let tmp = tempdir().unwrap();
        assert!(ProjectConfig::find_root(tmp.path()).unwrap().is_none());
    }
}
