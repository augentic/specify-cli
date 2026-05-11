//! Hub variant of `init`. Scaffolds a registry-only platform hub:
//! `registry.yaml` at the repo root plus `project.yaml { hub: true }`
//! under `.specify/`. Refuses to run when `.specify/` already exists
//! so it never clobbers a regular single-repo project.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use specify_capability::CacheMeta;
use specify_config::{LayoutExt, ProjectConfig};
use specify_error::{Error, is_kebab};
use specify_registry::Registry;

use crate::{InitOptions, InitResult, resolve_version, resolved_name, upsert_gitignore};

/// Sentinel value reported in [`InitResult::capability_name`] for hub
/// init. Hub `project.yaml` itself does **not** carry this string —
/// the absence of `capability:` together with `hub: true` is the
/// discriminator. The constant is kept solely so the JSON envelope
/// and the text response have a stable string to display.
const HUB_INIT_NAME: &str = "hub";

/// Scaffold a registry-only platform hub.
///
/// On-disk shape after success:
///
/// ```text
/// <project_dir>/
/// ├── registry.yaml     # { version: 1, projects: [] }
/// └── .specify/
///     └── project.yaml  # { name: …, hub: true }
/// ```
///
/// `registry.yaml` is the one platform-component artefact init
/// scaffolds — bootstrapping a hub *is* bootstrapping its registry.
/// `change.md` and `plan.yaml` stay operator-managed even on a hub;
/// the operator runs `specify change create <name>` and
/// `specify change plan create <name>` when the work itself begins.
///
/// Capability resolution is intentionally skipped — there is no
/// `pipeline.define` for a hub to walk.
///
/// # Errors
///
/// Returns an error if [`InitOptions::capability`] is set (mutually
/// exclusive with `--hub`), if the project name is not kebab-case, if
/// `.specify/` already exists, or if any filesystem write fails.
#[expect(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands an owned `InitOptions` to `init::run`, which forwards by value."
)]
pub(crate) fn run(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    if opts.capability.is_some() {
        return Err(Error::Diag {
            code: "init-requires-capability-or-hub",
            detail: "pass <capability> or --hub".to_string(),
        });
    }

    let layout = opts.project_dir.layout();
    let specify_dir = layout.specify_dir();
    if specify_dir.exists() {
        return Err(Error::Diag {
            code: "hub-init-specify-dir-exists",
            detail: format!(
                "init --hub: refusing to scaffold over an existing `.specify/` at {}; \
                 remove it first or run without --hub for a regular project",
                specify_dir.display()
            ),
        });
    }

    let name = resolved_name(opts.project_dir, opts.name);
    if !is_kebab(&name) {
        return Err(Error::Diag {
            code: "hub-init-name-not-kebab",
            detail: format!(
                "init --hub: project name `{name}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens). \
                 Pass --name <kebab-name> to override the directory basename."
            ),
        });
    }

    fs::create_dir_all(&specify_dir)?;
    let directories_created: Vec<PathBuf> = vec![specify_dir];

    let specify_version = resolve_version(opts.project_dir, opts.version_mode)?;

    let cfg = ProjectConfig {
        name,
        domain: opts.domain.map(str::to_string),
        capability: None,
        specify_version: Some(specify_version.clone()),
        rules: BTreeMap::new(),
        tools: Vec::new(),
        hub: true,
    };
    let config_path = layout.config_path();
    let serialised = serde_saphyr::to_string(&cfg)?;
    fs::write(&config_path, serialised)?;

    let registry = Registry {
        version: 1,
        projects: Vec::new(),
    };
    let registry_path = Registry::path(opts.project_dir);
    let registry_yaml = serde_saphyr::to_string(&registry)?;
    fs::write(&registry_path, registry_yaml)?;
    // Trivially passes for an empty list, but exercise the hub-mode
    // shape check so any future registry-write code paths inherit
    // the same invariant from this seed.
    registry.validate_shape_hub()?;

    upsert_gitignore(opts.project_dir)?;

    let cache_present = CacheMeta::path(opts.project_dir).exists();

    Ok(InitResult {
        config_path,
        capability_name: HUB_INIT_NAME.to_string(),
        cache_present,
        directories_created,
        scaffolded_rule_keys: Vec::new(),
        specify_version,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::{DateTime, Utc};
    use specify_config::ProjectConfig;
    use specify_registry::Registry;
    use tempfile::tempdir;

    use super::HUB_INIT_NAME;
    use crate::{InitOptions, VersionMode, init};

    fn fixed_now() -> DateTime<Utc> {
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
    }

    fn hub_opts<'a>(project_dir: &'a Path, name: &'a str) -> InitOptions<'a> {
        InitOptions {
            project_dir,
            capability: None,
            name: Some(name),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: true,
        }
    }

    #[test]
    fn hub_init_writes_canonical_on_disk_shape() {
        let tmp = tempdir().unwrap();
        let result = init(hub_opts(tmp.path(), "platform-hub"), fixed_now()).expect("hub init ok");

        let project_yaml = tmp.path().join(".specify/project.yaml");
        let registry_yaml = tmp.path().join("registry.yaml");
        assert!(project_yaml.is_file(), "project.yaml missing");
        assert!(registry_yaml.is_file(), "registry.yaml missing at repo root");

        // Hub init scaffolds `registry.yaml` (intrinsic to the hub's
        // purpose) but no other platform-component artefact.
        // `change.md` and `plan.yaml` stay operator-managed even on a hub.
        for absent in ["plan.yaml", "change.md"] {
            assert!(
                !tmp.path().join(absent).exists(),
                "hub init must not pre-touch `{absent}` at the repo root"
            );
        }

        // Phase-pipeline directories MUST NOT be scaffolded for a
        // hub — the absence of `capability:` (with `hub: true`) is
        // the discriminator that disables the define-build-merge
        // loop on the hub itself.
        assert!(!tmp.path().join(".specify/slices").exists());
        assert!(!tmp.path().join(".specify/specs").exists());
        assert!(!tmp.path().join(".specify/.cache").exists());

        let cfg = ProjectConfig::load(tmp.path()).expect("reload project.yaml");
        assert!(cfg.capability.is_none(), "hub project.yaml must omit capability:");
        assert!(cfg.hub, "project.yaml must carry hub: true");
        assert!(cfg.rules.is_empty(), "hubs do not scaffold rules");
        assert_eq!(cfg.name, "platform-hub");

        let on_disk = fs::read_to_string(&project_yaml).expect("read project.yaml");
        assert!(
            !on_disk.contains("capability:"),
            "hub project.yaml must omit `capability:`, got:\n{on_disk}"
        );
        assert!(
            !on_disk.contains("schema:"),
            "hub project.yaml must omit the legacy `schema:` field, got:\n{on_disk}"
        );
        assert!(
            on_disk.contains("hub: true"),
            "hub project.yaml must serialise `hub: true`, got:\n{on_disk}"
        );

        let registry = Registry::load(tmp.path()).expect("registry parses").expect("present");
        assert_eq!(registry.version, 1);
        assert!(registry.projects.is_empty(), "hub registry starts empty");

        assert_eq!(result.capability_name, HUB_INIT_NAME);
        assert!(result.scaffolded_rule_keys.is_empty());
    }

    #[test]
    fn hub_init_refuses_when_specify_dir_exists() {
        let tmp = tempdir().unwrap();
        // Pre-create `.specify/` with arbitrary content as if a regular
        // `specify init` had already run here.
        fs::create_dir_all(tmp.path().join(".specify")).unwrap();
        fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\ncapability: omnia\n")
            .unwrap();

        let err = init(hub_opts(tmp.path(), "platform-hub"), fixed_now())
            .expect_err("must refuse over existing dir");
        match err {
            specify_error::Error::Diag { code, detail } => {
                assert_eq!(code, "hub-init-specify-dir-exists");
                assert!(
                    detail.contains("refusing to scaffold"),
                    "diagnostic should explain the refusal, got: {detail}"
                );
                assert!(
                    detail.contains(".specify"),
                    "diagnostic should mention .specify, got: {detail}"
                );
            }
            other => panic!("wrong error variant: {other:?}"),
        }
        let on_disk = fs::read_to_string(tmp.path().join(".specify/project.yaml")).unwrap();
        assert_eq!(on_disk, "name: existing\ncapability: omnia\n");
    }

    #[test]
    fn hub_init_rejects_non_kebab_name() {
        let tmp = tempdir().unwrap();
        let err = init(hub_opts(tmp.path(), "BadName"), fixed_now()).expect_err("non-kebab name");
        match err {
            specify_error::Error::Diag { code, detail } => {
                assert_eq!(code, "hub-init-name-not-kebab");
                assert!(detail.contains("kebab-case"), "diagnostic should cite the rule: {detail}");
                assert!(
                    detail.contains("BadName"),
                    "diagnostic should echo the bad name: {detail}"
                );
            }
            other => panic!("wrong error variant: {other:?}"),
        }
        assert!(!tmp.path().join(".specify").exists(), "no .specify on validation failure");
    }
}
