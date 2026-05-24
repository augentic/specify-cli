//! `specify registry add` handler.

use specify_domain::registry::{Registry, RegistryProject};
use specify_domain::slice::atomic::yaml_write;
use specify_error::{Error, Result, is_kebab};

use super::dto::{AddBody, write_add_text};
use crate::context::Ctx;

pub(super) fn run(
    ctx: &Ctx, name: String, url: String, adapter: String, description: Option<String>,
) -> Result<()> {
    if !is_kebab(&name) {
        return Err(Error::Diag {
            code: "registry-add-name-not-kebab",
            detail: format!(
                "registry add: project name `{name}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
            ),
        });
    }
    if adapter.trim().is_empty() {
        return Err(Error::Diag {
            code: "registry-add-adapter-empty",
            detail: "registry add: --adapter must be non-empty (e.g. `omnia@v1`)".into(),
        });
    }

    let registry_path = Registry::path(&ctx.project_dir);
    let path = registry_path.display().to_string();
    let hub_mode = ctx.config.hub;
    let candidate = RegistryProject {
        name,
        url,
        adapter,
        description: description.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        contracts: None,
    };

    // `registry add` is "create or update": an absent `registry.yaml`
    // is synthesised from the canonical empty shape so the first
    // `add` against a fresh project succeeds without a separate
    // bootstrap step.
    let mut registry = Registry::load(&ctx.project_dir)?.unwrap_or_else(|| Registry {
        version: 1,
        projects: Vec::new(),
    });

    if registry.projects.iter().any(|p| p.name == candidate.name) {
        return Err(Error::Diag {
            code: "registry-add-name-duplicate",
            detail: format!("registry add: project `{}` already exists in {path}", candidate.name),
        });
    }

    let added = candidate.clone();
    registry.projects.push(candidate);

    // Surface validate_shape / validate_shape_hub errors verbatim —
    // their diagnostic codes (`description-missing-multi-repo`,
    // `hub-cannot-be-project`, etc.) are the documented contract.
    // Returning Err here aborts before the atomic write, so the
    // on-disk registry is never left in a shape-invalid state.
    if hub_mode {
        registry.validate_shape_hub()?;
    } else {
        registry.validate_shape()?;
    }

    yaml_write(&registry_path, &registry)?;

    ctx.write(
        &AddBody {
            registry,
            path,
            added,
        },
        write_add_text,
    )?;
    Ok(())
}
