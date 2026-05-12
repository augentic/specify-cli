//! Peer materialisation: turn registry entries into `.specify/workspace/<name>/`
//! symlinks (local URLs) or shallow Git clones (remote URLs).

use std::path::{Path, PathBuf};
use std::process::Command;

use specify_error::Error;

use super::bootstrap::{self, greenfield_init};
use super::git::{self, git_output_ok};
use super::slot_problem::inspect_at;
use super::{contracts_base, registry_symlink_target, workspace_base, workspace_slot_path};
use crate::registry::Registry;
use crate::registry::catalog::RegistryProject;
use crate::registry::gitignore::ensure_specify_gitignore_entries;

/// Materialise `.specify/workspace/<name>/` for every registry entry.
///
/// Symlinks for `.` / relative URLs, shallow `git clone` or `git fetch`
/// for remotes. Ensures `.gitignore` lists `.specify/workspace/` (and
/// `.specify/.cache/` when missing).
///
/// # Errors
///
/// Bubbles up registry parse failures, refused/mismatched slots, and any
/// per-project sync errors aggregated under `workspace-sync-failed`.
pub fn sync_all(project_dir: &Path) -> Result<(), Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(());
    };
    let projects = registry.select(&[])?;
    sync_projects(project_dir, &projects)
}

/// Materialise `.specify/workspace/<name>/` for selected registry entries.
///
/// Callers must pass projects returned by
/// [`Registry::select`] so unknown selectors fail before
/// this function performs filesystem, Git, or forge work.
///
/// # Errors
///
/// Aggregates per-project sync failures under the `workspace-sync-failed`
/// diagnostic; refuses materialisation when `.specify/workspace/` is itself
/// a symlink or otherwise invalid.
pub fn sync_projects(project_dir: &Path, projects: &[&RegistryProject]) -> Result<(), Error> {
    ensure_specify_gitignore_entries(project_dir)?;

    let base = prepare_workspace_base(project_dir)?;

    let mut errors: Vec<String> = Vec::new();
    for project in projects {
        let result = workspace_slot_path(&base, &project.name).and_then(|dest| {
            if let Some(problem) = inspect_at(project_dir, project, &dest) {
                return Err(Error::Diag {
                    code: "workspace-slot-mismatch",
                    detail: problem.message().to_string(),
                });
            }
            if project.is_local() {
                materialise_symlink(project_dir, &project.url, &dest)
            } else {
                materialise_git_remote(&project.url, &dest, &project.capability, project_dir)
            }
        });
        if let Err(err) = result {
            errors.push(format!("{}: {err}", project.name));
        }
    }

    let central_contracts = contracts_base(project_dir);
    if central_contracts.is_dir() {
        for project in projects {
            if project.is_local() {
                continue;
            }
            let Ok(slot) = workspace_slot_path(&base, &project.name) else {
                continue;
            };
            if !slot.is_dir() {
                continue;
            }
            if !slot.join(".specify").is_dir() {
                continue;
            }
            let dest_contracts = slot.join("contracts");
            if let Err(err) = distribute_contracts(&central_contracts, &dest_contracts) {
                errors.push(format!("{} (contracts): {err}", project.name));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Diag {
            code: "workspace-sync-failed",
            detail: format!(
                "workspace sync failed for {} project(s):\n{}",
                errors.len(),
                errors.join("\n")
            ),
        })
    }
}

fn prepare_workspace_base(project_dir: &Path) -> Result<PathBuf, Error> {
    let specify_dir = project_dir.join(".specify");
    reject_symlinked_directory(&specify_dir, ".specify/")?;
    std::fs::create_dir_all(&specify_dir).map_err(Error::Io)?;

    let base = workspace_base(project_dir);
    reject_symlinked_directory(&base, ".specify/workspace/")?;
    std::fs::create_dir_all(&base).map_err(Error::Io)?;
    reject_symlinked_directory(&base, ".specify/workspace/")?;

    Ok(base)
}

fn reject_symlinked_directory(path: &Path, label: &str) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Diag {
            code: "workspace-specify-dir-symlink",
            detail: format!(
                "{label} is a symlink; refusing to materialise workspace slots through it"
            ),
        }),
        Ok(meta) if !meta.is_dir() => Err(Error::Diag {
            code: "workspace-specify-dir-not-directory",
            detail: format!(
                "{label} exists but is not a directory; remove it before running workspace sync"
            ),
        }),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

pub(super) fn materialise_symlink(project_dir: &Path, url: &str, dest: &Path) -> Result<(), Error> {
    let target = registry_symlink_target(project_dir, url)?;

    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(dest) {
            Ok(resolved) if resolved == target => return Ok(()),
            Ok(resolved) => {
                return Err(Error::Diag {
                    code: "workspace-slot-symlink-target-mismatch",
                    detail: format!(
                        ".specify/workspace/{} already exists as a symlink to {}; expected {} from registry url `{url}`",
                        dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                        resolved.display(),
                        target.display()
                    ),
                });
            }
            Err(err) => {
                return Err(Error::Diag {
                    code: "workspace-slot-symlink-broken",
                    detail: format!(
                        ".specify/workspace/{} already exists as a broken symlink; expected {} from registry url `{url}` ({err})",
                        dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                        target.display()
                    ),
                });
            }
        },
        Ok(_) => {
            return Err(Error::Diag {
                code: "workspace-slot-not-symlink",
                detail: format!(
                    ".specify/workspace/{} already exists and is not a symlink; remove it before re-syncing",
                    dest.file_name().and_then(|s| s.to_str()).unwrap_or("?")
                ),
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(e)),
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }

    symlink(&target, dest)?;
    Ok(())
}

fn symlink(target: &Path, link: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(Error::Io)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).map_err(Error::Io)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(Error::Diag {
            code: "workspace-symlink-unsupported",
            detail: "platform does not support symlinks for `specify workspace sync`".to_string(),
        })
    }
}

pub(super) fn materialise_git_remote(
    url: &str, dest: &Path, capability: &str, initiating_project_dir: &Path,
) -> Result<(), Error> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Diag {
            code: "workspace-remote-slot-is-symlink",
            detail: format!(
                "`{}` is a symlink, but registry url `{url}` is remote-backed; remove the slot before re-syncing",
                dest.display()
            ),
        }),
        Ok(meta) if meta.is_dir() => {
            if !dest.join(".git").exists() {
                return Err(Error::Diag {
                    code: "workspace-remote-slot-not-git-clone",
                    detail: format!(
                        "`{}` exists but is not a git clone (no `.git/`); remove it or pick another registry name",
                        dest.display()
                    ),
                });
            }
            ensure_origin_matches(dest, url)?;
            if dest.join(".specify").join("project.yaml").exists() {
                git::run(
                    dest,
                    &["fetch", "--depth", "1"],
                    &format!("git fetch in {}", dest.display()),
                )
                .or(Ok(()))
            } else {
                greenfield_init(dest, capability, initiating_project_dir, true)
            }
        }
        Ok(_) => Err(Error::Diag {
            code: "workspace-slot-not-directory",
            detail: format!(
                "`{}` exists but is not a directory; remove it before re-syncing",
                dest.display()
            ),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }

            let clone_result = Command::new("git")
                .args(["clone", "--depth", "1", url])
                .arg(dest)
                .output()
                .map_err(|e| Error::Diag {
                    code: "workspace-git-clone-spawn-failed",
                    detail: format!(
                        "failed to spawn `git clone` for registry url `{url}`: {e} (is `git` installed?)"
                    ),
                })?;

            if clone_result.status.success() {
                ensure_origin_matches(dest, url)?;
                Ok(())
            } else {
                bootstrap::bootstrap(url, dest, capability, initiating_project_dir)
            }
        }
        Err(err) => Err(Error::Io(err)),
    }
}

fn ensure_origin_matches(dest: &Path, expected_url: &str) -> Result<(), Error> {
    match git_output_ok(dest, &["remote", "get-url", "origin"]) {
        Some(actual) if actual == expected_url => Ok(()),
        Some(actual) => Err(Error::Diag {
            code: "workspace-origin-mismatch",
            detail: format!(
                "`{}` origin remote is `{actual}`, but registry url is `{expected_url}`; \
                 remove the slot or update registry.yaml before re-syncing",
                dest.display()
            ),
        }),
        None => Err(Error::Diag {
            code: "workspace-origin-missing",
            detail: format!(
                "`{}` has no origin remote; expected registry url `{expected_url}`",
                dest.display()
            ),
        }),
    }
}

/// Copy root `contracts/` from the initiating repo into a workspace slot's
/// root `contracts/`. Removes the destination first for a clean replacement,
/// then copies recursively.
pub(super) fn distribute_contracts(src: &Path, dest: &Path) -> Result<(), Error> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| Error::Diag {
            code: "workspace-contracts-remove-failed",
            detail: format!("failed to remove old contracts at {}: {e}", dest.display()),
        })?;
    }
    copy_dir_recursive(src, dest)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dest).map_err(|e| Error::Diag {
        code: "workspace-contracts-create-failed",
        detail: format!("failed to create {}: {e}", dest.display()),
    })?;

    for entry in std::fs::read_dir(src).map_err(|e| Error::Diag {
        code: "workspace-contracts-read-failed",
        detail: format!("failed to read {}: {e}", src.display()),
    })? {
        let entry = entry.map_err(|e| Error::Diag {
            code: "workspace-contracts-entry-failed",
            detail: format!("dir entry error: {e}"),
        })?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            std::fs::copy(&src_path, &dest_path).map_err(|e| Error::Diag {
                code: "workspace-contracts-copy-failed",
                detail: format!(
                    "failed to copy {} to {}: {e}",
                    src_path.display(),
                    dest_path.display()
                ),
            })?;
        }
    }
    Ok(())
}
