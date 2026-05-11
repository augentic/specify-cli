//! Response DTOs for `specify registry *` handlers.

use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use specify_domain::registry::{Registry, RegistryProject};

use crate::output::{Render, display, serialize_path};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ShowBody {
    pub(super) registry: Option<Registry>,
    #[serde(serialize_with = "serialize_path")]
    pub(super) path: PathBuf,
}

impl Render for ShowBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let Some(reg) = self.registry.as_ref() else {
            return writeln!(w, "no registry declared at registry.yaml");
        };
        writeln!(w, "registry.yaml: {}", display(&self.path))?;
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
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ValidateBody {
    pub(super) registry: Option<Registry>,
    #[serde(serialize_with = "serialize_path")]
    pub(super) path: PathBuf,
    #[serde(skip)]
    pub(super) hub_mode: bool,
}

impl Render for ValidateBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let Some(reg) = self.registry.as_ref() else {
            return writeln!(w, "no registry declared at registry.yaml");
        };
        let count = reg.projects.len();
        if self.hub_mode {
            writeln!(w, "registry.yaml is well-formed in hub mode ({count} project(s))")
        } else {
            writeln!(w, "registry.yaml is well-formed ({count} project(s))")
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct AddBody {
    pub(super) registry: Registry,
    #[serde(serialize_with = "serialize_path")]
    pub(super) path: PathBuf,
    pub(super) added: RegistryProject,
}

impl Render for AddBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Added `{}` to {}", self.added.name, display(&self.path))?;
        writeln!(w, "registry now declares {} project(s)", self.registry.projects.len())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RemoveBody {
    pub(super) registry: Registry,
    #[serde(serialize_with = "serialize_path")]
    pub(super) path: PathBuf,
    pub(super) removed: String,
    pub(super) warnings: Vec<String>,
}

impl Render for RemoveBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Removed `{}` from {}", self.removed, display(&self.path))?;
        for warning in &self.warnings {
            writeln!(w, "warning: {warning}")?;
        }
        Ok(())
    }
}
