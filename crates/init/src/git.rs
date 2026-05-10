//! `git` invocations for fetching a capability from a GitHub URI.
//! Sparse-checkouts the capability's parent directory rather than the
//! full repository so the cache stays cheap on large monorepos.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use specify_error::Error;

pub fn sparse_checkout_github(
    repo_url: &str, checkout_ref: Option<&str>, capability_path: &str,
) -> Result<PathBuf, Error> {
    let checkout_dir = unique_temp_dir("specify-capability-checkout")?;
    let mut clone_args = vec!["clone", "--depth", "1", "--filter=blob:none", "--sparse"];
    if let Some(reference) = checkout_ref {
        clone_args.push("--branch");
        clone_args.push(reference);
    }
    clone_args.push(repo_url);
    let checkout_arg = checkout_dir.to_string_lossy().to_string();
    clone_args.push(&checkout_arg);
    cmd(&clone_args, "clone capability repository")?;

    let checkout_dir_arg = checkout_dir.to_string_lossy().to_string();
    let sparse_path = sparse_checkout_path(capability_path);
    cmd(
        &["-C", &checkout_dir_arg, "sparse-checkout", "set", "--", sparse_path],
        "sparse-checkout capability path",
    )?;
    Ok(checkout_dir)
}

fn sparse_checkout_path(capability_path: &str) -> &str {
    match capability_path.rsplit_once('/') {
        Some((parent, _name)) if !parent.is_empty() => parent,
        _ => capability_path,
    }
}

fn cmd(args: &[&str], action: &str) -> Result<(), Error> {
    let output = Command::new("git").args(args).output().map_err(|err| Error::Diag {
        code: "capability-git-spawn-failed",
        detail: format!("failed to spawn `git` to {action}: {err}"),
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Error::Diag {
        code: "capability-git-failed",
        detail: format!("git failed to {action}: {}", stderr.trim()),
    })
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf, Error> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| Error::Diag {
            code: "capability-clock-pre-epoch",
            detail: format!("system clock before unix epoch: {err}"),
        })?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_sparse_checkout_uses_capability_parent() {
        assert_eq!(sparse_checkout_path("capabilities/omnia"), "capabilities");
        assert_eq!(sparse_checkout_path("schemas/omnia"), "schemas");
        assert_eq!(sparse_checkout_path("omnia"), "omnia");
    }
}
