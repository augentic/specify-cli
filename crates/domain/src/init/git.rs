//! `git` invocations for fetching a capability from a GitHub URI.
//! Sparse-checkouts the capability's parent directory rather than the
//! full repository so the cache stays cheap on large monorepos.

use std::process::Command;

use specify_error::Error;
use tempfile::TempDir;

pub(super) fn sparse_checkout_github(
    repo_url: &str, checkout_ref: Option<&str>, capability_path: &str,
) -> Result<TempDir, Error> {
    let checkout = tempfile::Builder::new().prefix("specify-checkout-").tempdir()?;
    let checkout_arg = checkout.path().to_string_lossy().to_string();

    let mut clone_args = vec!["clone", "--depth", "1", "--filter=blob:none", "--sparse"];
    if let Some(reference) = checkout_ref {
        clone_args.push("--branch");
        clone_args.push(reference);
    }
    clone_args.push(repo_url);
    clone_args.push(&checkout_arg);
    cmd(&clone_args, "clone capability repository")?;

    let sparse_path = sparse_checkout_path(capability_path);
    cmd(
        &["-C", &checkout_arg, "sparse-checkout", "set", "--", sparse_path],
        "sparse-checkout capability path",
    )?;
    Ok(checkout)
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
