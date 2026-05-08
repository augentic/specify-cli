//! `ProjectConfig` — the in-memory model of `.specify/project.yaml` plus
//! the family of path helpers every subcommand reaches for when it needs
//! to locate `.specify/slices/`, `.specify/.cache/`, `.specify/archive/`,
//! or the operator-facing platform artifacts at the repo root
//! (`registry.yaml`, `plan.yaml`, `initiative.md`).
//!
//! Layout boundary: `.specify/` holds framework-managed state that the
//! CLI owns (configuration, working slices, archive, cache, workspace
//! clones, plan-authoring scratch, the advisory plan lock).
//! Operator-facing platform artifacts that are PR-reviewed and durable
//! live at the repo root. See [`docs/explanation/decision-log.md`](
//! ../../docs/explanation/decision-log.md) for the full rationale.
//!
//! RFC-13 §Migration invariant #3 ("concern-specific behaviour leaves
//! core") removed the per-class baseline helpers (`specs_dir`,
//! `contracts_dir`) that this module used to expose. Domain-specific
//! baseline locations are owned by the active capability and are
//! synthesised at the binary-side merge call site (currently
//! `src/commands/slice.rs::omnia_artifact_classes`). `ProjectConfig`
//! stays capability-agnostic.
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
    /// `hub` field for the discriminator. RFC-13 renamed this from the
    /// pre-RFC-13 `schema:` key; legacy files carrying `schema:` are
    /// rejected loudly with [`Error::SchemaBecameCapability`] in
    /// [`ProjectConfig::load`].
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

    /// `true` when this project is a registry-only **platform hub**
    /// (RFC-9 §1D, restated in RFC-13 §Migration). Hubs hold
    /// platform-level state — `registry.yaml`, `initiative.md`,
    /// `plan.yaml`, `workspace/` — but never appear in their own
    /// `registry.yaml` and have phase pipelines disabled. Hubs **omit**
    /// the `capability:` field entirely; the absence of `capability:`
    /// together with `hub: true` is the discriminator. Defaults to
    /// `false`; serialised only when `true` so non-hub `project.yaml`
    /// files round-trip byte-stable.
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
    ///   `Err(Error::SpecifyVersionTooOld { required, found })`.
    ///   Unparseable pinned versions are tolerated — we prefer a
    ///   permissive stance for a human-edited file.
    /// - Detects the pre-RFC-13 `schema:` key and refuses to load such a
    ///   `project.yaml` with [`Error::SchemaBecameCapability`]. RFC-13
    ///   renamed `project.yaml: schema:` to `project.yaml: capability:`;
    ///   the operator must rewrite the field (and re-run `specify init
    ///   <capability>` if they have not migrated yet).
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

        if has_legacy_schema_field(&text)? {
            return Err(Error::SchemaBecameCapability { path });
        }

        let cfg: Self = serde_saphyr::from_str(&text)?;

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
    ///
    /// Pre-RFC-13 projects used `.specify/changes/`; the on-disk
    /// migration to the new name lives at `specify migrate
    /// slice-layout` (RFC-13 chunk 3.6). Fresh `specify init` writes
    /// here from chunk 3.3 onward.
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

    /// Absolute path to `<project_dir>/plan.yaml` — the initiative
    /// plan. Platform-level artifact, lives at the repo root.
    #[must_use]
    pub fn plan_path(project_dir: &Path) -> PathBuf {
        project_dir.join("plan.yaml")
    }

    /// Absolute path to `<project_dir>/initiative.md` — the
    /// pre-Phase-3.7 operator brief filename. Retained so the
    /// v1-layout detector and `specify migrate change-noun` migrator
    /// have a single source of truth for the legacy basename;
    /// post-RFC-13-chunk-3.7 callers should use [`Self::change_brief_path`]
    /// instead.
    #[must_use]
    pub fn initiative_path(project_dir: &Path) -> PathBuf {
        project_dir.join(specify_capability::LEGACY_CHANGE_BRIEF_FILENAME)
    }

    /// Absolute path to `<project_dir>/change.md` — the umbrella
    /// operator brief at the repo root after RFC-13 chunk 3.7.
    /// Platform-level artifact; the post-RFC filename is canonical.
    #[must_use]
    pub fn change_brief_path(project_dir: &Path) -> PathBuf {
        project_dir.join(specify_capability::CHANGE_BRIEF_FILENAME)
    }

    /// Absolute path to `<project_dir>/.specify/.cache/`.
    #[must_use]
    pub fn cache_dir(project_dir: &Path) -> PathBuf {
        Self::specify_dir(project_dir).join(".cache")
    }

    /// Absolute path to `<project_dir>/.specify/archive/`. Not listed in
    /// RFC-1 §`config.rs` but needed by the merge engine; centralised
    /// here so there is still exactly one place the convention lives.
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

/// Scan `project_dir` for v1-layout artifacts that the CLI no longer reads.
///
/// The four legacy paths checked are `.specify/registry.yaml`,
/// `.specify/plan.yaml`, `.specify/initiative.md`, and
/// `.specify/contracts/`. Returns the repo-relative paths in
/// deterministic order so error output is stable. An empty `Vec` means
/// the project is on the v2 layout (or has neither shape, which is
/// also fine).
///
/// This is the engine behind the hard-cutover detector wired into
/// every project-aware CLI verb: when this returns non-empty the
/// dispatcher errors with [`Error::LegacyLayout`] and points the
/// operator at `specify migrate v2-layout`.
#[must_use]
pub fn detect_legacy_layout(project_dir: &Path) -> Vec<String> {
    let candidates: [(&str, bool); 4] = [
        (".specify/registry.yaml", project_dir.join(".specify/registry.yaml").is_file()),
        (".specify/plan.yaml", project_dir.join(".specify/plan.yaml").is_file()),
        (".specify/initiative.md", project_dir.join(".specify/initiative.md").is_file()),
        (".specify/contracts", project_dir.join(".specify/contracts").is_dir()),
    ];
    candidates
        .into_iter()
        .filter(|&(_, present)| present)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Detect whether `project_dir` lives below `.specify/workspace/<peer>/`.
///
/// This is a path-ancestry predicate only. RM-02 context generation uses
/// the shared posture to skip init-time `AGENTS.md` creation in workspace
/// clones and to refuse standalone generation there; callers that need a
/// fully initialized clone can layer `.specify/project.yaml` or plan-file
/// guards on top.
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

/// Probe a `project.yaml` text for the pre-RFC-13 `schema:` top-level
/// key without attempting to deserialise into [`ProjectConfig`]. We
/// route through `serde_json::Value` (the same shape `serde_saphyr` can
/// project a YAML document into) so the answer agrees with what
/// `serde_saphyr::from_str` would see — comments, document headers, and
/// quoted keys are all handled by the YAML parser, not by us.
///
/// Returns `Ok(true)` only when the document's top-level mapping
/// carries a `schema:` key. A missing `capability:` is *not* a trigger
/// on its own — hub projects legitimately omit it.
fn has_legacy_schema_field(text: &str) -> Result<bool, Error> {
    let value: serde_json::Value = serde_saphyr::from_str(text)?;
    Ok(value.as_object().is_some_and(|map| map.contains_key("schema")))
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
        assert_eq!(ProjectConfig::initiative_path(base), PathBuf::from("/a/b/initiative.md"));
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
            Error::SpecifyVersionTooOld { required, found } => {
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

        // Hub shape: no `capability:` field, just `hub: true`.
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
    fn load_refuses_legacy_schema_field_with_schema_became_capability() {
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\nschema: omnia\n");
        let err = ProjectConfig::load(tmp.path()).expect_err("legacy schema must be rejected");
        match err {
            Error::SchemaBecameCapability { path } => {
                assert!(path.ends_with(".specify/project.yaml"), "path: {}", path.display());
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn load_refuses_legacy_schema_hub_sentinel_with_schema_became_capability() {
        // Pre-RFC-13 hub shape: `schema: hub, hub: true`. The hub sentinel
        // is removed in Phase 1.3; loading still rejects loud.
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "name: demo\nschema: hub\nhub: true\n");
        let err = ProjectConfig::load(tmp.path()).expect_err("legacy hub shape must be rejected");
        assert!(matches!(err, Error::SchemaBecameCapability { .. }));
    }

    // ---- legacy-layout detector ---------------------------------------

    #[test]
    fn detect_legacy_returns_empty_for_clean_project() {
        let tmp = tempdir().unwrap();
        assert!(detect_legacy_layout(tmp.path()).is_empty());
    }

    #[test]
    fn detect_legacy_finds_each_v1_artifact() {
        let tmp = tempdir().unwrap();
        let specify = tmp.path().join(".specify");
        fs::create_dir_all(&specify).unwrap();
        fs::write(specify.join("registry.yaml"), "version: 1\nprojects: []\n").unwrap();
        fs::write(specify.join("plan.yaml"), "name: x\nchanges: []\n").unwrap();
        fs::write(specify.join("initiative.md"), "---\nname: x\n---\n").unwrap();
        fs::create_dir_all(specify.join("contracts").join("schemas")).unwrap();

        let found = detect_legacy_layout(tmp.path());
        assert_eq!(
            found,
            vec![
                ".specify/registry.yaml".to_string(),
                ".specify/plan.yaml".to_string(),
                ".specify/initiative.md".to_string(),
                ".specify/contracts".to_string(),
            ],
            "detector must surface every v1 artifact in deterministic order"
        );
    }

    #[test]
    fn detect_legacy_ignores_v2_layout() {
        let tmp = tempdir().unwrap();
        // v2: same files, but at the repo root.
        fs::write(tmp.path().join("registry.yaml"), "version: 1\nprojects: []\n").unwrap();
        fs::write(tmp.path().join("plan.yaml"), "name: x\nchanges: []\n").unwrap();
        fs::write(tmp.path().join("initiative.md"), "---\nname: x\n---\n").unwrap();
        fs::create_dir_all(tmp.path().join("contracts").join("schemas")).unwrap();

        assert!(detect_legacy_layout(tmp.path()).is_empty(), "v2 layout must not trigger");
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
}
