//! Greenfield bootstrap: scaffold a fresh workspace slot when a remote
//! clone fails or the slot has no `.specify/project.yaml`.

use std::path::Path;

use specify_error::Error;

use super::git::{self, git_output_ok, git_porcelain_non_empty};
use crate::gitignore::ensure_specify_gitignore_entries;

pub(super) fn bootstrap(
    url: &str, dest: &Path, capability: &str, initiating_project_dir: &Path,
) -> Result<(), Error> {
    std::fs::create_dir_all(dest).map_err(Error::Io)?;

    git::run(dest, &["init"], &format!("git init in {}", dest.display()))?;
    git::run(dest, &["remote", "add", "origin", url], &format!("git remote add origin {url}"))?;

    greenfield_init(dest, capability, initiating_project_dir, false)?;

    Ok(())
}

pub(super) fn greenfield_init(
    dest: &Path, capability: &str, initiating_project_dir: &Path, is_rerun: bool,
) -> Result<(), Error> {
    let capability = resolve_greenfield_capability(capability, initiating_project_dir)?;

    scaffold_greenfield_specify_tree(dest, &capability)?;

    git::run(dest, &["add", "."], &format!("git add in {}", dest.display()))?;

    if !git_porcelain_non_empty(dest) {
        return Ok(());
    }

    let has_commits = git_output_ok(dest, &["log", "--oneline", "-1"]).is_some();
    let commit_args = if is_rerun && has_commits {
        vec!["commit", "--amend", "--no-gpg-sign", "-m", "Initial Specify scaffold"]
    } else {
        vec!["commit", "--no-gpg-sign", "-m", "Initial Specify scaffold"]
    };
    git::run(dest, &commit_args, &format!("git commit in {}", dest.display()))?;

    Ok(())
}

fn scaffold_greenfield_specify_tree(dest: &Path, capability: &str) -> Result<(), Error> {
    let specify_dir = dest.join(".specify");
    for dir in [
        specify_dir.clone(),
        specify_dir.join("slices"),
        specify_dir.join("specs"),
        specify_dir.join("archive"),
        specify_dir.join(".cache"),
    ] {
        std::fs::create_dir_all(&dir).map_err(Error::Io)?;
    }

    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("greenfield");
    let project_yaml = format!(
        "name: {name}\ncapability: {capability}\nspecify_version: \"{}\"\nrules: {{}}\n",
        env!("CARGO_PKG_VERSION")
    );
    std::fs::write(specify_dir.join("project.yaml"), project_yaml).map_err(Error::Io)?;
    ensure_specify_gitignore_entries(dest)?;

    Ok(())
}

/// Resolve the capability identifier to pass into a greenfield slot's
/// `specify init <capability>`.
///
/// URL-shaped capabilities are already self-contained. Bare registry
/// capability identifiers are local to the initiating repo's cache, so
/// convert them into a file URI the spawned init can copy directly.
fn resolve_greenfield_capability(
    capability: &str, initiating_project_dir: &Path,
) -> Result<String, Error> {
    if capability.contains("://") {
        return Ok(capability.to_string());
    }
    let cache_base = initiating_project_dir.join(".specify").join(".cache");

    let direct = cache_base.join(capability);
    if direct.is_dir() {
        return Ok(format!("file://{}", direct.display()));
    }

    let without_ref = capability.split('@').next().unwrap_or(capability);
    if let Some(segment) = without_ref.rsplit('/').find(|s| !s.is_empty()) {
        let by_segment = cache_base.join(segment);
        if by_segment.is_dir() {
            return Ok(format!("file://{}", by_segment.display()));
        }
    }

    Err(Error::Diag {
        code: "workspace-capability-not-cached",
        detail: format!(
            "capability '{}' not cached in {}; run /spec:init in the initiating repo first",
            capability,
            cache_base.display()
        ),
    })
}
