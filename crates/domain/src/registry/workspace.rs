//! Multi-project workspace materialisation under `.specify/workspace/`
//! — bootstrap, sync, status, and push helpers for the slots derived
//! from `registry.yaml`.

mod bootstrap;
mod git;
mod push;
mod slot_problem;
mod status;
mod sync;

use std::path::{Component, Path, PathBuf};

pub use push::{PushOutcome, PushResult, github_slug, push_all, push_projects};
pub use slot_problem::{
    Problem as SlotProblem, Reason as SlotProblemReason, inspect as slot_problem,
};
use specify_error::Error;
pub use status::{ConfiguredTargetKind, SlotKind, SlotStatus, status, status_projects};
pub use sync::{sync_all, sync_projects};

fn workspace_base(project_dir: &Path) -> PathBuf {
    project_dir.join(".specify").join("workspace")
}

fn contracts_base(project_dir: &Path) -> PathBuf {
    project_dir.join("contracts")
}

fn local_target_path(project_dir: &Path, url: &str) -> PathBuf {
    if url == "." { project_dir.to_path_buf() } else { project_dir.join(url) }
}

fn workspace_slot_path(base: &Path, project_name: &str) -> Result<PathBuf, Error> {
    let name_path = Path::new(project_name);
    let mut components = name_path.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(slot_escape_error(project_name));
    };
    if components.next().is_some() || component.to_string_lossy() != project_name {
        return Err(slot_escape_error(project_name));
    }

    let dest = base.join(project_name);
    if dest.strip_prefix(base).ok() != Some(Path::new(project_name)) {
        return Err(slot_escape_error(project_name));
    }
    Ok(dest)
}

fn slot_escape_error(project_name: &str) -> Error {
    Error::Diag {
        code: "workspace-slot-name-invalid",
        detail: format!(
            "registry project name `{project_name}` would escape `.specify/workspace/<project>/`; \
             project names must be a single path component"
        ),
    }
}

fn registry_symlink_target(project_dir: &Path, url: &str) -> Result<PathBuf, Error> {
    if url == "." {
        std::fs::canonicalize(project_dir).map_err(|e| Error::Diag {
            code: "workspace-registry-url-unresolved",
            detail: format!("could not resolve project directory for registry url `.`: {e}"),
        })
    } else {
        let joined = project_dir.join(url);
        std::fs::canonicalize(&joined).map_err(|e| Error::Diag {
            code: "workspace-registry-url-unresolved",
            detail: format!(
                "could not resolve registry url `{url}` relative to {}: {}",
                project_dir.display(),
                e
            ),
        })
    }
}
