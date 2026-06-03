//! Re-entry (`specify init --upgrade`) body: bumps `project.yaml.specify_version`
//! to the running binary over an existing `.specify/` without re-scaffolding.
//! Mutates only `project.yaml`; never touches slices, specs, archive, registry,
//! or the adapter cache.
//!
//! One runner serves both regular and workspace projects: the
//! preservation logic is identical, so the dispatcher routes here ahead
//! of the workspace / regular branch.

use std::fs;

use specify_error::Error;

use crate::config::{Layout, ProjectConfig};
use crate::init::adapter_uri::adapter_name_from_value;
use crate::init::cache::{CacheMeta, CodexMeta};
use crate::init::{InitOptions, InitResult, resolve_version};

/// Run the re-entry version bump.
///
/// Loads the existing config through the migration carve-out
/// ([`ProjectConfig::load_for_migration`], which never raises
/// [`Error::ProjectNeedsMigration`]), refuses if a migration is owed,
/// then bumps `specify_version` to the running binary's version — but
/// only when it differs, so an already-current project is a true no-op
/// (no `project.yaml` write).
///
/// # Errors
///
/// - [`Error::NotInitialized`] when `.specify/project.yaml` is absent —
///   `--upgrade` requires an existing project.
/// - [`Error::CliTooOld`] when the pinned floor is newer than this
///   binary (propagated by the loader).
/// - [`Error::ProjectNeedsMigration`] when the pinned major is older
///   than this binary's (exit 4 — the operator must run `specify
///   migrate` first). Dormant while the binary is pre-1.0: the
///   migration tuple is always `None` at major `0`.
/// - filesystem / serialisation errors from rewriting `project.yaml`.
pub(super) fn run(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    let (mut cfg, migration) = ProjectConfig::load_for_migration(opts.project_dir)?;
    if let Some((from, to)) = migration {
        return Err(Error::ProjectNeedsMigration { from, to });
    }

    let layout = Layout::new(opts.project_dir);
    let config_path = layout.config_path();
    let target = resolve_version();

    let specify_version_changed = cfg.specify_version.as_deref() != Some(target.as_str());
    if specify_version_changed {
        cfg.specify_version = Some(target.clone());
        let serialised = serde_saphyr::to_string(&cfg)?;
        fs::write(&config_path, serialised)?;
    }

    let adapter_name = if cfg.workspace {
        "workspace".to_string()
    } else {
        cfg.adapter
            .as_deref()
            .map_or_else(String::new, |value| adapter_name_from_value(value).to_string())
    };

    Ok(InitResult {
        config_path,
        adapter_name,
        cache_present: CacheMeta::path(opts.project_dir).exists(),
        codex_present: CodexMeta::path(opts.project_dir).exists(),
        directories_created: Vec::new(),
        scaffolded_rule_keys: Vec::new(),
        specify_version: target,
        specify_version_changed,
        wasm_pkg_config_written: false,
    })
}

#[cfg(test)]
mod tests;
