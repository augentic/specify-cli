//! `specify registry remove` handler.

use std::path::Path;

use specify_change::Plan;
use specify_config::{LayoutExt, with_existing_state};
use specify_error::{Error, Result};
use specify_registry::Registry;

use super::dto::RemoveBody;
use crate::context::Ctx;

pub(super) fn run(ctx: &Ctx, name: String) -> Result<()> {
    let path = Registry::path(&ctx.project_dir);
    let hub_mode = ctx.config.hub;

    // Pre-flight: surface the legacy `registry-remove-no-registry`
    // diagnostic when the file is absent. `with_existing_state` would
    // emit the generic `Error::ArtifactNotFound`; the registry-specific
    // diag is part of the wire contract.
    if !path.exists() {
        return Err(Error::Diag {
            code: "registry-remove-no-registry",
            detail: format!("registry remove: no registry declared at {}", path.display()),
        });
    }

    let body =
        with_existing_state::<Registry, _, _>(ctx.layout(), "registry.yaml", move |registry| {
            let position =
                registry.projects.iter().position(|p| p.name == name).ok_or_else(|| {
                    Error::Diag {
                        code: "registry-remove-not-found",
                        detail: format!(
                            "registry remove: project `{name}` not found in {}",
                            path.display()
                        ),
                    }
                })?;
            registry.projects.remove(position);

            // A removal can only relax the multi-repo description
            // invariant, so the post-write check should always
            // succeed; we run it anyway to pin the contract.
            if hub_mode {
                registry.validate_shape_hub()?;
            } else {
                registry.validate_shape()?;
            }

            let warnings = plan_refs(&ctx.project_dir, &name);
            Ok(RemoveBody {
                registry: registry.clone(),
                path,
                removed: name,
                warnings,
            })
        })?;

    ctx.out().write(&body)?;
    Ok(())
}

/// Scan `plan.yaml` (when present) for plan entries whose `project`
/// field equals `removed`. Returns one human-readable warning per
/// affected entry. Best-effort: any parse error is surfaced as a
/// single advisory string instead of failing the remove (the registry
/// write has already landed, so the operator needs to learn about
/// both halves).
pub(super) fn plan_refs(project_dir: &Path, removed: &str) -> Vec<String> {
    let plan_path = project_dir.layout().plan_path();
    if !plan_path.exists() {
        return Vec::new();
    }
    match Plan::load(&plan_path) {
        Ok(plan) => {
            let referencing: Vec<&str> = plan
                .entries
                .iter()
                .filter(|entry| entry.project.as_deref() == Some(removed))
                .map(|entry| entry.name.as_str())
                .collect();
            if referencing.is_empty() {
                Vec::new()
            } else {
                vec![format!(
                    "plan.yaml has {n} entry(ies) still referencing project `{removed}`: {entries}. \
                     Run `specify change plan amend <change> --project <other>` to rewire them.",
                    n = referencing.len(),
                    entries = referencing.join(", "),
                )]
            }
        }
        Err(err) => vec![format!(
            "plan.yaml present but unreadable; cannot check for stale references to `{removed}`: {err}"
        )],
    }
}
