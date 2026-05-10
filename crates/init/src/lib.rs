#![allow(
    clippy::multiple_crate_versions,
    reason = "ProjectConfig re-exports `specify_tool::Tool`, which transitively pulls in Wasmtime/WASI duplicate versions."
)]

//! `init` — the orchestration called by `specify init`.
//!
//! Scaffolds `.specify/{slices,specs,archive,.cache}/`, resolves the
//! requested capability into `.specify/.cache/`, writes
//! `.specify/project.yaml` with a `rules:` key scaffolded from the
//! capability's `pipeline.define` briefs, and upserts the
//! `.specify/.cache/` and `.specify/workspace/` lines into the project
//! `.gitignore`. Idempotent: a second call with the same options
//! refreshes the cache and rewrites `project.yaml` byte-for-byte.
//!
//! `init` writes only the per-project skeleton. Repo-root artefacts
//! (`registry.yaml`, `change.md`, `plan.yaml`) are minted by their
//! own verbs (`specify registry add`, `specify change create`,
//! `specify change plan create`) and never pre-touched here.
//!
//! Hub mode ([`InitOptions::hub`] = `true`) is the one exception: it
//! scaffolds an empty `registry.yaml` alongside a sentinel
//! `project.yaml { hub: true }` (with `capability:` omitted) and
//! refuses to run when `.specify/` already exists.

mod cache;
mod capability_uri;
mod git;
mod hub;
mod regular;

use std::fs;
use std::path::{Path, PathBuf};

use specify_config::ProjectConfig;
use specify_error::Error;

/// Inputs to [`init`]. Borrow-shaped so callers (the CLI and tests) can
/// build the struct without cloning path buffers.
pub struct InitOptions<'a> {
    /// Root of the project being initialised.
    pub project_dir: &'a Path,
    /// Capability identifier (bare name like `omnia` or a URL) to fetch
    /// or copy into `.specify/.cache/`. Required for regular init; must
    /// be `None` when [`InitOptions::hub`] is `true` (hubs do not
    /// resolve a capability at init time).
    pub capability: Option<&'a str>,
    /// Project name; defaults to the project directory name when `None`.
    pub name: Option<&'a str>,
    /// Optional project domain description.
    pub domain: Option<&'a str>,
    /// Controls what `specify_version` gets written into `project.yaml`.
    pub version_mode: VersionMode,
    /// When `true`, scaffold a registry-only platform **hub** instead
    /// of a regular project: writes `registry.yaml` at the repo root
    /// and `project.yaml { hub: true }` (with `capability:` omitted)
    /// under `.specify/`. Hub init refuses to run when `.specify/`
    /// already exists so it never clobbers a regular single-repo
    /// project.
    pub hub: bool,
}

/// How `init` determines the `specify_version` floor in `project.yaml`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VersionMode {
    /// Write the running binary's version as the floor (fresh init and
    /// `init --upgrade`).
    WriteCurrent,
    /// Preserve the existing `specify_version` in `project.yaml` when
    /// present (reinitialize flow).
    Preserve,
}

/// Structured summary of what `init` did, returned for downstream
/// rendering by both the JSON and text CLI paths.
#[derive(Debug, Clone)]
pub struct InitResult {
    /// Path to the written `project.yaml`.
    pub config_path: PathBuf,
    /// Resolved capability name from the capability root. For hub init
    /// this is the literal `"hub"` so the JSON envelope stays stable
    /// for downstream consumers.
    pub capability_name: String,
    /// Whether `.specify/.cache/cache_meta.yaml` exists.
    pub cache_present: bool,
    /// Directories that were newly created (empty on re-init).
    pub directories_created: Vec<PathBuf>,
    /// Brief IDs scaffolded into the `rules:` map.
    pub scaffolded_rule_keys: Vec<String>,
    /// The `specify_version` value written into `project.yaml`.
    pub specify_version: String,
}

/// Initialise `.specify/` inside `opts.project_dir`.
///
/// Idempotent: a second call with identical options succeeds, creates no
/// new directories, doesn't duplicate the `.gitignore` entry, and writes
/// byte-identical `project.yaml` contents.
///
/// When [`InitOptions::hub`] is `true`, dispatches to the private hub
/// runner for the platform-hub on-disk shape.
///
/// # Errors
///
/// Pre-condition: regular (non-hub) init requires
/// [`InitOptions::capability`] to be set; the CLI dispatcher enforces
/// the `init-requires-capability-or-hub` invariant ahead of this call,
/// but `init` re-validates as a defence in depth. Bubbles up
/// filesystem, capability resolution, and serialisation errors from
/// the underlying calls.
#[allow(clippy::needless_pass_by_value)]
pub fn init(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    if opts.hub {
        return hub::run(opts);
    }
    regular::run(opts)
}

pub(crate) fn resolved_name(project_dir: &Path, explicit: Option<&str>) -> String {
    if let Some(explicit) = explicit {
        return explicit.to_string();
    }
    project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| "project".to_string(), str::to_string)
}

pub(crate) fn resolve_version(project_dir: &Path, mode: VersionMode) -> Result<String, Error> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    if matches!(mode, VersionMode::WriteCurrent) {
        return Ok(current);
    }

    // Preserve: keep the existing value when `project.yaml` already
    // carries one. Reading the file directly avoids re-running the
    // version-floor check inside `ProjectConfig::load` (which would
    // reject the load when the existing floor is newer than the
    // running binary — but `Preserve` is meant precisely for that
    // case).
    let path = ProjectConfig::config_path(project_dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(current),
        Err(err) => return Err(Error::Io(err)),
    };
    let existing: ProjectConfig = serde_saphyr::from_str(&text)?;
    Ok(existing.specify_version.unwrap_or(current))
}

pub(crate) fn upsert_gitignore(project_dir: &Path) -> Result<(), Error> {
    specify_registry::ensure_specify_gitignore_entries(project_dir)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn regular_init_rejects_missing_capability() {
        let tmp = tempdir().unwrap();
        let err = init(InitOptions {
            project_dir: tmp.path(),
            capability: None,
            name: Some("demo"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        })
        .expect_err("missing capability must error");
        assert!(matches!(err, Error::InitNeedsCapability), "got: {err:?}");
    }

    #[test]
    fn hub_init_rejects_capability_argument() {
        // `--hub` and `<capability>` are mutually exclusive; the
        // orchestrator re-checks even when the CLI layer already
        // filtered.
        let tmp = tempdir().unwrap();
        let err = init(InitOptions {
            project_dir: tmp.path(),
            capability: Some("omnia"),
            name: Some("platform-hub"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: true,
        })
        .expect_err("hub + capability must error");
        assert!(matches!(err, Error::InitNeedsCapability), "got: {err:?}");
    }
}
