//! Orchestration for `specify init`. Scaffolds `.specify/`, resolves
//! the requested capability, writes `project.yaml`, and upserts
//! `.gitignore` lines. Hub mode additionally mints `registry.yaml`.

mod cache;
mod capability_uri;
mod git;
mod hub;
mod regular;

use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use specify_error::Error;
use specify_tool::{DEFAULT_WASM_PKG_CONFIG, WASM_PKG_CONFIG_FILENAME};

use crate::config::{Layout, ProjectConfig};

/// Inputs to [`init`]. Borrow-shaped so callers (the CLI and tests) can
/// build the struct without cloning path buffers.
#[derive(Debug)]
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
    /// `true` when this run wrote `.specify/wasm-pkg.toml` for the
    /// first time; `false` when an operator-edited file was preserved.
    /// Lets the JSON envelope distinguish a fresh scaffold from a
    /// re-init that left registry config intact.
    pub wasm_pkg_config_written: bool,
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
/// `now` records the `cache_meta.yaml::fetched_at` stamp; the dispatcher
/// passes `Timestamp::now` and tests pin a deterministic value.
///
/// # Errors
///
/// Pre-condition: regular (non-hub) init requires
/// [`InitOptions::capability`] to be set; the CLI dispatcher enforces
/// the `init-requires-capability-or-hub` invariant ahead of this call,
/// but `init` re-validates as a defence in depth. Bubbles up
/// filesystem, capability resolution, and serialisation errors from
/// the underlying calls.
pub fn init(opts: InitOptions<'_>, now: Timestamp) -> Result<InitResult, Error> {
    if opts.hub {
        return hub::run(opts);
    }
    regular::run(opts, now)
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
    let path = Layout::new(project_dir).config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(current),
        Err(err) => return Err(Error::Io(err)),
    };
    let existing: ProjectConfig = serde_saphyr::from_str(&text)?;
    Ok(existing.specify_version.unwrap_or(current))
}

pub(crate) fn upsert_gitignore(project_dir: &Path) -> Result<(), Error> {
    crate::registry::ensure_specify_gitignore_entries(project_dir)
}

/// Scaffold the project-local wasm-pkg config when absent, preserving
/// any operator-edited file byte-for-byte on re-init.
///
/// The contents are the canonical RFC-17 mapping
/// (`specify -> augentic.io`); see
/// [`specify_tool::DEFAULT_WASM_PKG_CONFIG`]. Operators are expected
/// to edit this file to add private mirrors or other namespace
/// mappings, so a re-init must never clobber their changes.
///
/// Returns `Ok(true)` when this call wrote the file, `Ok(false)` when
/// it already existed.
///
/// # Errors
///
/// Propagates filesystem errors from creating `.specify/` or writing
/// the file.
pub(crate) fn scaffold_wasm_pkg_config(layout: &Layout<'_>) -> Result<bool, Error> {
    let specify_dir = layout.specify_dir();
    let path = specify_dir.join(WASM_PKG_CONFIG_FILENAME);
    if path.exists() {
        return Ok(false);
    }
    fs::create_dir_all(&specify_dir)?;
    fs::write(&path, DEFAULT_WASM_PKG_CONFIG)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn fixed_now() -> Timestamp {
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
    }

    #[test]
    fn regular_init_rejects_missing_capability() {
        let tmp = tempdir().unwrap();
        let err = init(
            InitOptions {
                project_dir: tmp.path(),
                capability: None,
                name: Some("demo"),
                domain: None,
                version_mode: VersionMode::WriteCurrent,
                hub: false,
            },
            fixed_now(),
        )
        .expect_err("missing capability must error");
        assert!(
            matches!(
                &err,
                Error::Diag {
                    code: "init-requires-capability-or-hub",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn hub_init_rejects_capability_argument() {
        // `--hub` and `<capability>` are mutually exclusive; the
        // orchestrator re-checks even when the CLI layer already
        // filtered.
        let tmp = tempdir().unwrap();
        let err = init(
            InitOptions {
                project_dir: tmp.path(),
                capability: Some("omnia"),
                name: Some("platform-hub"),
                domain: None,
                version_mode: VersionMode::WriteCurrent,
                hub: true,
            },
            fixed_now(),
        )
        .expect_err("hub + capability must error");
        assert!(
            matches!(
                &err,
                Error::Diag {
                    code: "init-requires-capability-or-hub",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }
}
