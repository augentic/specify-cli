//! `ProjectConfig` — in-memory model of `.specify/project.yaml` — and
//! `Layout<'a>`, the typed home for every `.specify/` and repo-root
//! path helper the CLI reaches for.

mod atomic;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use atomic::{AtomicYaml, with_state};
use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::platform::Platform;

/// In-memory representation of `.specify/project.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProjectConfig {
    /// Project name (defaults to the project directory name at init time).
    pub name: String,

    /// Free-text description of the project's tech stack, architecture,
    /// and testing approach. Falls back to the adapter's domain when empty.
    ///
    /// Authored intent only. A project's *derived* routing identity —
    /// the `surface[]` of owned units and a `recent[]` merge tail — is
    /// projected from its baseline (`.specify/specs/` + journal), never
    /// re-authored here. Unknown facets such as `capabilities` /
    /// `keywords` are silently ignored (this struct does not
    /// `deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Adapter identifier — either a bare name (`omnia`) or a URL.
    /// Absent for registry-only workspaces (`workspace: true`); see the
    /// `workspace` field for the discriminator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,

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
    /// points owned by `specify-tool`, not by any adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<specify_tool::manifest::Tool>,

    /// Target platforms this project builds for (e.g. `core`, `ios`,
    /// `android`). Set at `specify init --platforms` and changeable via
    /// `specify init --upgrade --platforms`. When the bound target
    /// adapter declares `platforms.required`, this field must be
    /// non-empty and must include `Platform::Core`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<Platform>,

    /// `true` when this project is a registry-only **workspace**.
    /// Workspaces hold platform-level state — `registry.yaml`,
    /// `change.md`, `plan.yaml`, workspace slots under `.specify/workspace/`
    /// — but never appear in their own `registry.yaml` and have phase
    /// pipelines disabled. Workspaces **omit** the `adapter:` field
    /// entirely; the absence of `adapter:` together with `workspace: true`
    /// is the discriminator. Defaults to `false`; serialised only when
    /// `true` so regular `project.yaml` files round-trip byte-stable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
}
impl ProjectConfig {
    /// Load `.specify/project.yaml` from `project_dir`.
    ///
    /// Enforces the `specify_version` floor: a pinned version newer than
    /// `CARGO_PKG_VERSION` is rejected, but an unparseable pin is
    /// tolerated — we prefer a permissive stance for a human-edited file.
    ///
    /// # Errors
    ///
    /// - [`Error::NotInitialized`] if `.specify/project.yaml` is absent.
    /// - [`Error::Io`] if the file exists but cannot be read.
    /// - [`Error::YamlDe`] if the file is not valid project YAML.
    /// - [`Error::CliTooOld`] if the pinned `specify_version` floor is
    ///   newer than this binary's version.
    pub fn load(project_dir: &Path) -> Result<Self, Error> {
        let path = Layout::new(project_dir).config_path();
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

        if let Some(pinned) = &cfg.specify_version
            && let Some((from, to)) = needs_migration(current, pinned)
        {
            return Err(Error::ProjectNeedsMigration { from, to });
        }

        Ok(cfg)
    }

    /// Bootstrap carve-out for the migration-aware commands
    /// (`specify migrate` / `specify upgrade` / `specify init --upgrade`).
    ///
    /// Performs the same read + parse + [`Error::CliTooOld`] floor check as
    /// [`ProjectConfig::load`], but instead of raising
    /// [`Error::ProjectNeedsMigration`] it *returns* the parsed config plus
    /// the `(from, to)` migration tuple (the `needs_migration` result;
    /// `None` when no migration is required). This is the only legal way to
    /// observe a project that is itself in the "needs migration" state, since
    /// those commands exist precisely to resolve it.
    ///
    /// # Errors
    ///
    /// - [`Error::NotInitialized`] if `.specify/project.yaml` is absent.
    /// - [`Error::Io`] if the file exists but cannot be read.
    /// - [`Error::YamlDe`] if the file is not valid project YAML.
    /// - [`Error::CliTooOld`] if the pinned `specify_version` floor is
    ///   newer than this binary's version.
    pub fn load_for_migration(
        project_dir: &Path,
    ) -> Result<(Self, Option<(String, String)>), Error> {
        let path = Layout::new(project_dir).config_path();
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

        let migration =
            cfg.specify_version.as_deref().and_then(|pinned| needs_migration(current, pinned));

        Ok((cfg, migration))
    }

    /// Walk `start_dir` and its ancestors looking for the first directory
    /// that contains `.specify/project.yaml`. Returns `None` when no
    /// ancestor is initialised. Filesystem probe errors are treated as
    /// "this candidate isn't initialised" — the next ancestor is tried.
    #[must_use]
    pub fn find_root(start_dir: &Path) -> Option<PathBuf> {
        start_dir
            .ancestors()
            .find(|candidate| Layout::new(candidate).config_path().try_exists().unwrap_or(false))
            .map(Path::to_path_buf)
    }

    /// Resolve a `rules` value to an absolute path under `.specify/`.
    /// Returns `None` when the brief has no override (absent or empty).
    #[must_use]
    pub fn rule_path(&self, project_dir: &Path, brief_id: &str) -> Option<PathBuf> {
        let value = self.rules.get(brief_id)?;
        if value.is_empty() {
            return None;
        }
        Some(Layout::new(project_dir).specify_dir().join(value))
    }
}

/// Typed view over a project root that exposes every `.specify/` and
/// repo-root path helper as an inherent method.
///
/// Construct with [`Layout::new`]. The newtype concentrates the
/// `.specify/` boundary in one place: callers never join
/// `.specify/...` literally; they ask the layout for the directory
/// they want.
#[derive(Debug, Clone, Copy)]
pub struct Layout<'a> {
    project_dir: &'a Path,
}

impl<'a> Layout<'a> {
    /// Wrap `project_dir` as the typed root for path lookups.
    #[must_use]
    pub const fn new(project_dir: &'a Path) -> Self {
        Self { project_dir }
    }

    /// Project root the layout is anchored at.
    #[must_use]
    pub const fn project_dir(&self) -> &'a Path {
        self.project_dir
    }

    /// Absolute path to `<project_dir>/.specify/`.
    #[must_use]
    pub fn specify_dir(&self) -> PathBuf {
        self.project_dir.join(".specify")
    }

    /// Absolute path to `<project_dir>/.specify/project.yaml`.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.specify_dir().join("project.yaml")
    }

    /// Absolute path to `<project_dir>/.specify/slices/`.
    #[must_use]
    pub fn slices_dir(&self) -> PathBuf {
        self.specify_dir().join(crate::slice::SLICES_DIR_NAME)
    }

    /// Absolute path to `<project_dir>/.specify/topology.lock` — the
    /// committed projection of each member project's `project.yaml`
    /// topology facets, regenerated by `specify workspace sync`.
    /// Machine-written; never hand-edited.
    #[must_use]
    pub fn topology_lock_path(&self) -> PathBuf {
        self.specify_dir().join("topology.lock")
    }

    /// Absolute path to `<project_dir>/.specify/cache/`.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.specify_dir().join("cache")
    }

    /// Absolute path to `<project_dir>/.specify/decisions/` — the
    /// append-only Decision Record catalogue promoted by
    /// `specify slice merge`. One flat, project-global tree of
    /// `DEC-NNNN-<slug>.md` files. Machine-written by merge; the single
    /// permitted post-write mutation is a supersede status flip.
    #[must_use]
    pub fn decisions_dir(&self) -> PathBuf {
        self.specify_dir().join("decisions")
    }

    /// Absolute path to `<project_dir>/.specify/archive/`. Centralised
    /// here so there is exactly one place the convention lives.
    #[must_use]
    pub fn archive_dir(&self) -> PathBuf {
        self.specify_dir().join("archive")
    }

    /// Absolute path to `<project_dir>/.specify/.migrate/<kind>/` — the
    /// per-migrator scratch root the migration framework owns. `kind`
    /// is a stable `crate::migrate::MigrationKind::id` (e.g. `v2-to-v3`).
    #[must_use]
    pub fn migrate_dir(&self, kind: &str) -> PathBuf {
        self.specify_dir().join(".migrate").join(kind)
    }

    /// Absolute path to `<project_dir>/.specify/.migrate/<kind>/staging/`
    /// — where a migrator stages file writes before renaming them into
    /// place atomically.
    #[must_use]
    pub fn migrate_staging_dir(&self, kind: &str) -> PathBuf {
        self.migrate_dir(kind).join("staging")
    }

    /// Absolute path to `<project_dir>/registry.yaml` — the platform
    /// catalogue. Platform-level artifact, lives at the repo root.
    #[must_use]
    pub fn registry_path(&self) -> PathBuf {
        self.project_dir.join("registry.yaml")
    }

    /// Absolute path to `<project_dir>/plan.yaml` — the change
    /// plan. Platform-level artifact, lives at the repo root.
    #[must_use]
    pub fn plan_path(&self) -> PathBuf {
        self.project_dir.join("plan.yaml")
    }

    /// Absolute path to `<project_dir>/change.md` — the umbrella
    /// operator brief at the repo root. Platform-level artifact.
    #[must_use]
    pub fn change_brief_path(&self) -> PathBuf {
        self.project_dir.join("change.md")
    }

    /// Absolute path to `<project_dir>/discovery.md` — the candidate
    /// inventory written at `/spec:plan`'s survey step and read during
    /// lead reconciliation.
    #[must_use]
    pub fn discovery_path(&self) -> PathBuf {
        self.project_dir.join("discovery.md")
    }
}

/// Detect whether `project_dir` lives below `.specify/workspace/<peer>/`.
///
/// This is a path-ancestry predicate only. Context generation uses the
/// shared posture to skip init-time `AGENTS.md` creation in materialised
/// slots and to refuse standalone generation there; callers that need
/// a fully initialized slot can layer `.specify/project.yaml` or
/// plan-file guards on top.
#[must_use]
pub fn is_slot(project_dir: &Path) -> bool {
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

/// Parse the major version component; `None` for unparseable input.
fn major(v: &str) -> Option<u64> {
    semver::Version::parse(v).ok().map(|x| x.major)
}

/// Returns `Some((from, to))` when `pinned`'s major is strictly older than
/// `current`'s, signalling that a migration must run before the CLI can
/// operate. Unparseable versions yield `None` (permissive, matching the
/// [`version_is_older`] stance). Dormant while the binary is pre-1.0.
fn needs_migration(current: &str, pinned: &str) -> Option<(String, String)> {
    match (major(pinned), major(current)) {
        (Some(from), Some(to)) if to > from => Some((pinned.to_string(), current.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
