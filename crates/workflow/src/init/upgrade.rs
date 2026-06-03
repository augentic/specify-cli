//! Re-entry (`specrun init --upgrade`) init body. Bumps
//! `project.yaml.specify_version` to the running binary's version over
//! an already-populated `.specify/` without re-scaffolding (RFC-30
//! §D5, Wave E).
//!
//! The write set is closed: this runner mutates **only**
//! `project.yaml`, rewriting `specify_version` and preserving every
//! other field (including `adapter:` / `hub:`). It never touches
//! `slices/`, `specs/`, `archive/`, `registry.yaml`,
//! `.specify/design-system/*`, or the adapter cache, and it never
//! re-fetches the cache — preservation holds by construction because
//! nothing here resolves an adapter. `AGENTS.md` regeneration (only
//! when absent) is owned by the command layer's
//! `generate_initial_context`, mirroring the fresh-init path.
//!
//! One runner serves both regular and hub projects: the preservation
//! logic is identical, so the dispatcher routes here ahead of the
//! hub / regular branch (an intentional deviation from the plan's
//! literal "route through regular/hub" wording — sharing the runner is
//! simpler and avoids duplicating the same bump twice).

use std::fs;

use specify_error::Error;

use crate::adapter::TargetAdapter;
use crate::config::{Layout, ProjectConfig};
use crate::init::adapter_uri::adapter_name_from_value;
use crate::init::cache::{CacheMeta, CodexMeta};
use crate::init::{InitOptions, InitResult, resolve_version, validate_platforms};

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
///   than this binary's (exit 4 — the operator must run `specrun
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

    let platforms_changed = if let Some(incoming) = opts.platforms {
        let adapter_name =
            cfg.adapter.as_deref().map(adapter_name_from_value).ok_or_else(|| Error::Diag {
                code: "upgrade-platforms-no-adapter",
                detail: "--platforms requires a project with a bound target adapter (hub projects \
                         have no adapter)"
                    .to_string(),
            })?;
        let resolved = TargetAdapter::resolve(adapter_name, opts.project_dir)?;
        let validated =
            validate_platforms(Some(incoming), resolved.manifest.platforms.as_ref(), adapter_name)?;
        let changed = cfg.platforms != validated;
        cfg.platforms = validated;
        changed
    } else {
        false
    };

    let specify_version_changed = cfg.specify_version.as_deref() != Some(target.as_str());
    let needs_write = specify_version_changed || platforms_changed;
    if specify_version_changed {
        cfg.specify_version = Some(target.clone());
    }
    if needs_write {
        let serialised = serde_saphyr::to_string(&cfg)?;
        fs::write(&config_path, serialised)?;
    }

    let adapter_name = if cfg.hub {
        "hub".to_string()
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
