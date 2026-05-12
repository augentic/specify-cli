//! `specify registry add` handler.

use specify_domain::config::{InitPolicy, with_state};
use specify_domain::registry::{Registry, RegistryProject};
use specify_error::{Error, Result, is_kebab};

use super::dto::{AddBody, write_add_text};
use crate::context::Ctx;

pub(super) fn run(
    ctx: &Ctx, name: String, url: String, capability: String, description: Option<String>,
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
    if capability.trim().is_empty() {
        return Err(Error::Diag {
            code: "registry-add-capability-empty",
            detail: "registry add: --capability must be non-empty (e.g. `omnia@v1`)".into(),
        });
    }

    let path = Registry::path(&ctx.project_dir);
    let hub_mode = ctx.config.hub;
    let candidate = RegistryProject {
        name,
        url,
        capability,
        description: description.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        }),
        contracts: None,
    };

    let body =
        with_state::<Registry, _, _>(ctx.layout(), InitPolicy::CreateMissing, move |registry| {
            if registry.projects.iter().any(|p| p.name == candidate.name) {
                return Err(Error::Diag {
                    code: "registry-add-name-duplicate",
                    detail: format!(
                        "registry add: project `{}` already exists in {}",
                        candidate.name,
                        path.display()
                    ),
                });
            }

            registry.projects.push(candidate);

            // Surface validate_shape / validate_shape_hub errors verbatim —
            // their diagnostic codes (`description-missing-multi-repo`,
            // `hub-cannot-be-project`, etc.) are the documented contract.
            // Returning Err here aborts `with_state` before the atomic
            // write, so the on-disk registry is never left in a
            // shape-invalid state.
            if hub_mode {
                registry.validate_shape_hub()?;
            } else {
                registry.validate_shape()?;
            }

            let added = registry
                .projects
                .last()
                .expect("we just pushed an entry; non-empty by construction")
                .clone();
            Ok(AddBody {
                registry: registry.clone(),
                path,
                added,
            })
        })?;

    ctx.write(&body, write_add_text)?;
    Ok(())
}
