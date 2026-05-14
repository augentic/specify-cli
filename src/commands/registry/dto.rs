//! Response DTOs for `specify registry *` handlers.

use std::io::Write;

use serde::Serialize;
use specify_domain::registry::{Registry, RegistryProject};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ShowBody {
    pub(super) registry: Option<Registry>,
    pub(super) path: String,
}

pub(super) fn write_show_text(w: &mut dyn Write, body: &ShowBody) -> std::io::Result<()> {
    let Some(reg) = body.registry.as_ref() else {
        return writeln!(w, "no registry declared at registry.yaml");
    };
    writeln!(w, "registry.yaml: {}", body.path)?;
    writeln!(w, "version: {}", reg.version)?;
    if reg.projects.is_empty() {
        return writeln!(w, "projects: (none)");
    }
    writeln!(w, "projects:")?;
    for project in &reg.projects {
        writeln!(w, "  - name: {}", project.name)?;
        writeln!(w, "    url: {}", project.url)?;
        writeln!(w, "    capability: {}", project.capability)?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ValidateBody {
    pub(super) registry: Option<Registry>,
    pub(super) path: String,
    #[serde(skip)]
    pub(super) hub_mode: bool,
}

pub(super) fn write_validate_text(w: &mut dyn Write, body: &ValidateBody) -> std::io::Result<()> {
    let Some(reg) = body.registry.as_ref() else {
        return writeln!(w, "no registry declared at registry.yaml");
    };
    let count = reg.projects.len();
    if body.hub_mode {
        writeln!(w, "registry.yaml is well-formed in hub mode ({count} project(s))")
    } else {
        writeln!(w, "registry.yaml is well-formed ({count} project(s))")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct AddBody {
    pub(super) registry: Registry,
    pub(super) path: String,
    pub(super) added: RegistryProject,
}

pub(super) fn write_add_text(w: &mut dyn Write, body: &AddBody) -> std::io::Result<()> {
    writeln!(w, "Added `{}` to {}", body.added.name, body.path)?;
    writeln!(w, "registry now declares {} project(s)", body.registry.projects.len())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RemoveBody {
    pub(super) registry: Registry,
    pub(super) path: String,
    pub(super) removed: String,
    pub(super) warnings: Vec<String>,
}

pub(super) fn write_remove_text(w: &mut dyn Write, body: &RemoveBody) -> std::io::Result<()> {
    writeln!(w, "Removed `{}` from {}", body.removed, body.path)?;
    for warning in &body.warnings {
        writeln!(w, "warning: {warning}")?;
    }
    Ok(())
}
