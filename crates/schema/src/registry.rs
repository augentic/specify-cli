//! Registry parser — platform-level catalogue of peer projects
//! (RFC-3a §*The Registry*).
//!
//! `.specify/registry.yaml` enumerates the repos that comprise the
//! platform and the schema each of them uses. The file is optional:
//! an absent or single-entry registry is equivalent to single-repo
//! mode. Multi-entry registries activate the `/spec:plan` *sync
//! peers* phase — but that behaviour lands in C28/C30; this module
//! only handles shape parsing and validation.
//!
//! No JSON schema file ships for v1 per the RFC — the shape is
//! enforced directly by [`Registry::validate_shape`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// In-memory representation of `.specify/registry.yaml`.
///
/// `additionalProperties: false` is expressed via
/// `#[serde(deny_unknown_fields)]` — the same posture the `plan.yaml`
/// `ScopeShape` uses — so typos (e.g. `versions:`, `project:`) fail
/// fast at parse time rather than silently round-tripping.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    /// Schema version. `1` is the only accepted value for this
    /// release; [`Registry::validate_shape`] rejects anything else
    /// with an actionable diagnostic.
    pub version: u32,
    /// Platform catalogue. Empty or single-entry is equivalent to
    /// "single-repo mode"; multi-entry activates the `/spec:plan`
    /// *sync peers* phase (C28/C30).
    #[serde(default)]
    pub projects: Vec<RegistryProject>,
}

/// One entry in [`Registry::projects`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryProject {
    /// Kebab-case identifier for the project. Obeys the same
    /// naming rules as change names
    /// (`specify_change::actions::validate_name`) — duplicated here
    /// because `specify-schema` sits upstream of `specify-change` in
    /// the crate graph.
    pub name: String,
    /// Clone target — `.`, a repo-relative path (`../peer`, `./foo`,
    /// `pkg/sub`), `git@host:path`, or an `http(s)://`, `ssh://`, or
    /// `git+http(s)://` / `git+ssh://` remote. Shape-validated by
    /// [`Registry::validate_shape`] (RFC-3a C28). Stored verbatim.
    pub url: String,
    /// Schema identifier — e.g. `omnia@v1`. Opaque at this layer;
    /// the `name@version` suffix is **not** parsed here.
    pub schema: String,
    /// Domain-level characterisation of the project (RFC-3b).
    /// Required when `len(projects) > 1`; optional for single-project registries.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional contract role declarations for this project (RFC-8 Layer 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contracts: Option<ContractRoles>,
}

/// Contract role declarations for a registry project.
/// All fields are optional — a project may only produce, only consume,
/// or have no contract relationships at all.
///
/// RFC-12 collapsed the role set to two: `produces` (this project
/// authoritatively implements the contract) and `consumes` (this project
/// calls or subscribes to the contract). A contract that no project
/// produces is, by definition, externally authored — no separate
/// `imports` field is needed to mark it. `#[serde(deny_unknown_fields)]`
/// causes any surviving `imports:` key in `registry.yaml` to fail at
/// parse time, which is the documented migration trigger (RFC-12
/// §Migration).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractRoles {
    /// Contract files this project is the authoritative implementer of.
    /// Paths relative to root `contracts/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: Vec<String>,
    /// Contract files this project calls or subscribes to as a client.
    /// Paths relative to root `contracts/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumes: Vec<String>,
}

impl Registry {
    /// Absolute path to `.specify/registry.yaml` for a given project
    /// directory.
    #[must_use]
    pub fn path(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify").join("registry.yaml")
    }

    /// Load + shape-validate the registry.
    ///
    /// - `Ok(None)` — the file is absent. The registry is optional
    ///   and a missing file is *not* an error.
    /// - `Ok(Some(_))` — file parsed and shape-validated.
    /// - `Err(_)` — malformed YAML, unknown keys, wrong `version`,
    ///   kebab-case / required-field / duplicate-name violations.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(project_dir: &Path) -> Result<Option<Self>, Error> {
        let path = Self::path(project_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|err| Error::Config(format!("failed to read {}: {err}", path.display())))?;
        let registry: Self = serde_saphyr::from_str(&content)
            .map_err(|err| Error::Config(format!("registry.yaml: invalid YAML: {err}")))?;
        registry.validate_shape()?;
        Ok(Some(registry))
    }

    /// Enforce invariants that serde cannot express on its own:
    /// `version == 1`, kebab-case project names, non-empty required
    /// strings, unique project names, and well-formed [`RegistryProject::url`]
    /// values (RFC-3a C28). Returns the first error encountered — the
    /// convention used elsewhere in `specify-schema` for fast-fail shape
    /// validation.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    #[allow(clippy::too_many_lines)]
    pub fn validate_shape(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::Config(format!(
                "registry.yaml: unsupported version {}; v1 is the only accepted value",
                self.version
            )));
        }

        let mut seen_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (idx, project) in self.projects.iter().enumerate() {
            if project.name.is_empty() {
                return Err(Error::Config(format!("registry.yaml: projects[{idx}].name is empty")));
            }
            if !is_kebab_case(&project.name) {
                return Err(Error::Config(format!(
                    "registry.yaml: projects[{idx}].name `{}` must be kebab-case \
                     (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)",
                    project.name
                )));
            }
            if project.url.is_empty() {
                return Err(Error::Config(format!(
                    "registry.yaml: projects[{idx}] (`{}`).url is empty",
                    project.name
                )));
            }
            validate_project_url(&project.url, idx, &project.name)?;
            if project.schema.is_empty() {
                return Err(Error::Config(format!(
                    "registry.yaml: projects[{idx}] (`{}`).schema is empty",
                    project.name
                )));
            }
            if !seen_names.insert(project.name.as_str()) {
                return Err(Error::Config(format!(
                    "registry.yaml: duplicate project name `{}`",
                    project.name
                )));
            }
        }

        if self.projects.len() > 1 {
            for (idx, project) in self.projects.iter().enumerate() {
                let missing = project.description.as_ref().is_none_or(|s| s.trim().is_empty());
                if missing {
                    return Err(Error::Config(format!(
                        "registry.yaml: projects[{idx}] (`{}`).description is required when the registry declares more than one project (description-missing-multi-repo)",
                        project.name
                    )));
                }
            }
        }

        // --- Contract role invariants (RFC-8 Layer 2; RFC-12 collapsed
        // the role set to `produces` + `consumes` and dropped the
        // produce/import mutual-exclusion check) ---

        // Invariant 3: Path validity — no absolute or `..` paths.
        for project in &self.projects {
            if let Some(ref roles) = project.contracts {
                for path in roles.produces.iter().chain(roles.consumes.iter()) {
                    if path.starts_with('/') || path.contains("..") {
                        return Err(Error::Config(format!(
                            "registry.yaml: contract path '{}' in project '{}' must be relative (no '..' or absolute paths)",
                            path, project.name
                        )));
                    }
                }
            }
        }

        // Invariant 4: Self-consistency — a project must not list the
        // same path in both `produces` and `consumes`.
        for project in &self.projects {
            if let Some(ref roles) = project.contracts {
                let produced: HashSet<&str> =
                    roles.produces.iter().map(std::string::String::as_str).collect();
                for path in &roles.consumes {
                    if produced.contains(path.as_str()) {
                        return Err(Error::Config(format!(
                            "registry.yaml: project '{}' lists '{}' in both 'produces' and 'consumes'",
                            project.name, path
                        )));
                    }
                }
            }
        }

        // Invariant 1: Single producer — each contract path appears in
        // `produces` for at most one project.
        let mut producers: HashMap<&str, &str> = HashMap::new();
        for project in &self.projects {
            if let Some(ref roles) = project.contracts {
                for path in &roles.produces {
                    if let Some(existing) = producers.get(path.as_str()) {
                        return Err(Error::Config(format!(
                            "registry.yaml: contract path '{}' is produced by both '{}' and '{}'",
                            path, existing, project.name
                        )));
                    }
                    producers.insert(path, &project.name);
                }
            }
        }

        Ok(())
    }

    /// `true` when the registry declares at most one project.
    ///
    /// Absent registry + single-entry registry behave identically in
    /// the `/spec:plan` flow (RFC-3a §*When are `registry.yaml` and
    /// `initiative.md` required?*). Useful to C28/C30 where the
    /// *sync peers* phase is gated on `len(projects) > 1`.
    #[must_use]
    pub const fn is_single_repo(&self) -> bool {
        self.projects.len() <= 1
    }

    /// Hub-only shape check (RFC-9 §1D).
    ///
    /// Runs the base [`Registry::validate_shape`] first, then layers on
    /// the additional invariant that a **registry-only platform hub**
    /// must never list itself as a project: any entry with `url: .` is
    /// rejected with a `hub-cannot-be-project` diagnostic. The hub
    /// holds platform-level state (registry, initiative brief, plan,
    /// workspace clones) but is never a code project.
    ///
    /// Callers opt in by checking `project.yaml:hub: true` and
    /// invoking this method in addition to (or instead of) the base
    /// [`Registry::validate_shape`]. Non-hub callers continue to use
    /// the base method unchanged — this is a strictly additive API.
    ///
    /// # Errors
    ///
    /// Returns the first base-shape error if `validate_shape` fails,
    /// or a `hub-cannot-be-project` config error if any entry's `url`
    /// equals `.`.
    pub fn validate_shape_hub(&self) -> Result<(), Error> {
        self.validate_shape()?;
        for (idx, project) in self.projects.iter().enumerate() {
            if project.url == "." {
                return Err(Error::Config(format!(
                    "registry.yaml: projects[{idx}] (`{}`).url is `.`; \
                     a registry-only platform hub must not appear in its own \
                     registry — code projects always live in their own repos \
                     (hub-cannot-be-project)",
                    project.name
                )));
            }
        }
        Ok(())
    }
}

impl RegistryProject {
    /// `true` when this entry's [`RegistryProject::url`] should be
    /// materialised under `.specify/workspace/<name>/` as a symlink to a
    /// resolved filesystem path (`.` or a repo-relative path), as opposed
    /// to a `git clone` remote.
    ///
    /// Callers may assume [`Registry::validate_shape`] has already accepted
    /// the URL — this predicate mirrors the C28 classification rules.
    #[must_use]
    pub fn url_materialises_as_symlink(&self) -> bool {
        self.url == "." || (!self.url.contains("://") && !self.url.starts_with("git@"))
    }
}

/// RFC-3a C28 — reject malformed `projects[].url` while accepting:
/// `.`, repo-relative paths, `http(s)://`, `git@host:path`, `ssh://`,
/// and `git+http(s)://` / `git+ssh://` forms.
fn validate_project_url(url: &str, idx: usize, project_name: &str) -> Result<(), Error> {
    const ALLOWED_SCHEMES: &[&str] = &["http", "https", "ssh", "git+https", "git+http", "git+ssh"];

    if url.trim().is_empty() {
        return Err(Error::Config(format!(
            "registry.yaml: projects[{idx}] (`{project_name}`).url is empty or whitespace-only"
        )));
    }
    if url != url.trim() {
        return Err(Error::Config(format!(
            "registry.yaml: projects[{idx}] (`{project_name}`).url must not have leading or trailing whitespace"
        )));
    }

    if url == "." {
        return Ok(());
    }

    if url.starts_with("git@") {
        return Ok(());
    }

    if let Some(pos) = url.find("://") {
        let scheme = &url[..pos];
        if !ALLOWED_SCHEMES.contains(&scheme) {
            return Err(Error::Config(format!(
                "registry.yaml: projects[{idx}] (`{project_name}`).url has unsupported URL scheme `{scheme}`: \
                 expected one of http, https, ssh, git+https, git+http, git+ssh, a `git@host:path` remote, `.`, or a relative path"
            )));
        }
        return Ok(());
    }

    if url.contains(':') {
        return Err(Error::Config(format!(
            "registry.yaml: projects[{idx}] (`{project_name}`).url `{url}` is not valid: \
             ':' is only allowed in `git@host:path` remotes or in `scheme://` URLs"
        )));
    }

    if url.starts_with('/') {
        return Err(Error::Config(format!(
            "registry.yaml: projects[{idx}] (`{project_name}`).url must be a relative path, `.`, or a remote URL — absolute filesystem paths are not allowed"
        )));
    }

    #[cfg(windows)]
    if looks_like_windows_drive_path(url) {
        return Err(Error::Config(format!(
            "registry.yaml: projects[{idx}] (`{project_name}`).url must be a relative path, `.`, or a remote URL — absolute Windows paths are not allowed"
        )));
    }

    Ok(())
}

#[cfg(windows)]
fn looks_like_windows_drive_path(url: &str) -> bool {
    let mut chars = url.chars();
    let Some(c) = chars.next() else {
        return false;
    };
    c.is_ascii_alphabetic() && chars.next() == Some(':')
}

/// Kebab-case predicate shared within `specify-schema`. Identical
/// contract to `specify_change::actions::validate_name`; duplicated
/// because `specify-schema` is upstream of `specify-change` in the
/// crate dep graph and cannot call through.
pub fn is_kebab_case(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') {
        return false;
    }
    if s.contains("--") {
        return false;
    }
    s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
