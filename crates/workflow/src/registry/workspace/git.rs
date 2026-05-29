//! Thin shells around `git` for workspace materialisation, status, and push.

use std::path::Path;
use std::process::Command;

use specify_error::Error;

pub(super) fn git_output_ok(tree: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(tree).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub(super) fn git_porcelain_non_empty(tree: &Path) -> bool {
    let Ok(output) =
        Command::new("git").arg("-C").arg(tree).args(["status", "--porcelain"]).output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    !output.stdout.is_empty()
}

pub(super) fn run(cwd: &Path, args: &[&str], label: &str) -> Result<(), Error> {
    let output = Command::new("git")
        .args(["-c", "user.name=Specify", "-c", "user.email=specify@example.invalid"])
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| Error::Diag {
            code: "workspace-git-spawn-failed",
            detail: format!("{label}: failed to spawn git: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Diag {
            code: "workspace-git-command-failed",
            detail: format!("{label} failed: {stderr}"),
        });
    }
    Ok(())
}

pub(super) fn git_status_porcelain(project_path: &Path) -> Result<String, Error> {
    git_stdout_allow_empty(
        project_path,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        "git status --porcelain",
    )
}

pub(super) fn git_stdout_trimmed(
    project_path: &Path, args: &[&str], label: &str,
) -> Result<String, Error> {
    let stdout = git_stdout_allow_empty(project_path, args, label)?;
    let trimmed = stdout.trim().to_string();
    if trimmed.is_empty() {
        return Err(Error::Diag {
            code: "workspace-git-empty-output",
            detail: format!("{label} returned no output"),
        });
    }
    Ok(trimmed)
}

pub(super) fn git_stdout_allow_empty(
    project_path: &Path, args: &[&str], label: &str,
) -> Result<String, Error> {
    let output =
        Command::new("git").arg("-C").arg(project_path).args(args).output().map_err(|err| {
            Error::Diag {
                code: "workspace-git-spawn-failed",
                detail: format!("{label}: failed to spawn git: {err}"),
            }
        })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    Err(Error::Diag {
        code: "workspace-git-command-failed",
        detail: format!("{label} failed: {}", String::from_utf8_lossy(&output.stderr).trim()),
    })
}
