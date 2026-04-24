//! Multi-project workspace materialisation under `.specify/workspace/`
//! (RFC-3a C29).

use std::path::Path;
use std::process::Command;

use specify_change::Plan;
use specify_error::Error;
use specify_schema::Registry;

use crate::config::ProjectConfig;
use crate::init::ensure_specify_gitignore_entries;

/// Materialise `.specify/workspace/<name>/` for every registry entry:
/// symlinks for `.` / relative URLs, shallow `git clone` or `git fetch`
/// for remotes. Ensures `.gitignore` lists `.specify/workspace/` (and
/// `.specify/.cache/` when missing).
pub fn sync_registry_workspace(project_dir: &Path) -> Result<(), Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(());
    };

    ensure_specify_gitignore_entries(project_dir)?;

    let base = ProjectConfig::specify_dir(project_dir).join("workspace");
    std::fs::create_dir_all(&base)?;

    let mut errors: Vec<String> = Vec::new();
    for project in &registry.projects {
        let dest = base.join(&project.name);
        let result = if project.url_materialises_as_symlink() {
            materialise_symlink(project_dir, &project.url, &dest)
        } else {
            materialise_git_remote(&project.url, &dest, &project.schema, project_dir)
        };
        if let Err(err) = result {
            errors.push(format!("{}: {err}", project.name));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "workspace sync failed for {} project(s):\n{}",
            errors.len(),
            errors.join("\n")
        )))
    }
}

/// One row for `specify workspace status` text/JSON output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSlotStatus {
    /// Registry project name (`.specify/workspace/<name>/`).
    pub name: String,
    /// How the slot is materialised on disk.
    pub kind: WorkspaceSlotKind,
    /// `git rev-parse HEAD` when the resolved tree is a git checkout.
    pub head_sha: Option<String>,
    /// `true` when `git status --porcelain` is non-empty.
    pub dirty: Option<bool>,
}

/// Classification of a workspace slot on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSlotKind {
    /// Path missing.
    Missing,
    /// Symlink under `.specify/workspace/<name>/`.
    Symlink,
    /// Ordinary directory with a `.git/` metadata tree (clone target).
    GitClone,
    /// Present but neither a recognised symlink nor a git work tree.
    Other,
}

/// Inspect `.specify/workspace/<name>/` for each registry project.
///
/// Returns `Ok(None)` when `.specify/registry.yaml` is absent.
pub fn workspace_status(project_dir: &Path) -> Result<Option<Vec<WorkspaceSlotStatus>>, Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(None);
    };

    let base = ProjectConfig::specify_dir(project_dir).join("workspace");
    let mut out = Vec::with_capacity(registry.projects.len());
    for project in &registry.projects {
        let slot = base.join(&project.name);
        out.push(describe_slot(&project.name, &slot));
    }
    Ok(Some(out))
}

fn describe_slot(name: &str, slot: &Path) -> WorkspaceSlotStatus {
    let meta = match std::fs::symlink_metadata(slot) {
        Ok(m) => m,
        Err(_) => {
            return WorkspaceSlotStatus {
                name: name.to_string(),
                kind: WorkspaceSlotKind::Missing,
                head_sha: None,
                dirty: None,
            };
        }
    };

    if meta.file_type().is_symlink() {
        let (head_sha, dirty) =
            if slot.exists() { git_head_and_dirty_for_tree(slot) } else { (None, None) };
        return WorkspaceSlotStatus {
            name: name.to_string(),
            kind: WorkspaceSlotKind::Symlink,
            head_sha,
            dirty,
        };
    }

    if meta.is_dir() && slot.join(".git").exists() {
        let (head_sha, dirty) = git_head_and_dirty_for_tree(slot);
        return WorkspaceSlotStatus {
            name: name.to_string(),
            kind: WorkspaceSlotKind::GitClone,
            head_sha,
            dirty,
        };
    }

    WorkspaceSlotStatus {
        name: name.to_string(),
        kind: WorkspaceSlotKind::Other,
        head_sha: None,
        dirty: None,
    }
}

fn git_head_and_dirty_for_tree(tree: &Path) -> (Option<String>, Option<bool>) {
    let head = git_output_ok(tree, &["rev-parse", "HEAD"]);
    let dirty = head.as_ref().map(|_| git_porcelain_non_empty(tree));
    (head, dirty)
}

fn git_output_ok(tree: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(tree).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn git_porcelain_non_empty(tree: &Path) -> bool {
    let output =
        match Command::new("git").arg("-C").arg(tree).args(["status", "--porcelain"]).output() {
            Ok(o) => o,
            Err(_) => return false,
        };
    if !output.status.success() {
        return false;
    }
    !output.stdout.is_empty()
}

fn materialise_symlink(project_dir: &Path, url: &str, dest: &Path) -> Result<(), Error> {
    let target = if url == "." {
        std::fs::canonicalize(project_dir).map_err(|e| {
            Error::Config(format!(
                "could not resolve project directory for registry url `.`: {}",
                e
            ))
        })?
    } else {
        let joined = project_dir.join(url);
        std::fs::canonicalize(&joined).map_err(|e| {
            Error::Config(format!(
                "could not resolve registry url `{url}` relative to {}: {}",
                project_dir.display(),
                e
            ))
        })?
    };

    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(dest) {
            Ok(resolved) if resolved == target => return Ok(()),
            Ok(_) => {
                return Err(Error::Config(format!(
                    ".specify/workspace/{} already exists as a symlink pointing elsewhere (expected {})",
                    dest.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    target.display()
                )));
            }
            Err(_) => {
                std::fs::remove_file(dest).map_err(Error::Io)?;
            }
        },
        Ok(_) => {
            return Err(Error::Config(format!(
                ".specify/workspace/{} already exists and is not a symlink; remove it before re-syncing",
                dest.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            )));
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
        Err(Error::Config("platform does not support symlinks for `specify workspace sync`".into()))
    }
}

fn materialise_git_remote(
    url: &str, dest: &Path, schema: &str, initiating_project_dir: &Path,
) -> Result<(), Error> {
    if dest.exists() {
        if dest.join(".git").is_dir() {
            if dest.join(".specify").join("project.yaml").exists() {
                // Healthy clone or complete greenfield bootstrap — refresh
                run_git(
                    dest,
                    &["fetch", "--depth", "1"],
                    &format!("git fetch in {}", dest.display()),
                )
                .or(Ok(()))
            } else {
                // Partial greenfield bootstrap: .git/ present but .specify/project.yaml absent
                greenfield_init(dest, schema, initiating_project_dir, true)
            }
        } else {
            Err(Error::Config(format!(
                "`{}` exists but is not a git clone (no `.git/`); remove it or pick another registry name",
                dest.display()
            )))
        }
    } else {
        // Attempt clone
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }

        let clone_result = Command::new("git")
            .args(["clone", "--depth", "1", url])
            .arg(dest)
            .output()
            .map_err(|e| {
                Error::Config(format!(
                    "failed to spawn `git clone` for registry url `{url}`: {e} (is `git` installed?)"
                ))
            })?;

        if clone_result.status.success() {
            Ok(())
        } else {
            // Clone failed — treat as greenfield
            greenfield_bootstrap(url, dest, schema, initiating_project_dir)
        }
    }
}

/// Full greenfield bootstrap: mkdir, git init, git remote add, specify init, git add+commit.
fn greenfield_bootstrap(
    url: &str, dest: &Path, schema: &str, initiating_project_dir: &Path,
) -> Result<(), Error> {
    std::fs::create_dir_all(dest).map_err(Error::Io)?;

    run_git(dest, &["init"], &format!("git init in {}", dest.display()))?;
    run_git(dest, &["remote", "add", "origin", url], &format!("git remote add origin {url}"))?;

    greenfield_init(dest, schema, initiating_project_dir, false)?;

    Ok(())
}

/// Run `specify init` in a greenfield slot, then git add + commit.
/// `is_rerun` controls whether we amend the commit or create a new one.
fn greenfield_init(
    dest: &Path, schema: &str, initiating_project_dir: &Path, is_rerun: bool,
) -> Result<(), Error> {
    let schema_dir = locate_schema_cache(schema, initiating_project_dir)?;

    let status = Command::new("specify")
        .arg("init")
        .arg(schema)
        .arg("--schema-dir")
        .arg(&schema_dir)
        .current_dir(dest)
        .status()
        .map_err(|e| {
            Error::Config(format!(
                "failed to spawn `specify init` for greenfield project at {}: {e}",
                dest.display()
            ))
        })?;

    if !status.success() {
        return Err(Error::Config(format!(
            "`specify init {schema} --schema-dir {}` failed in {}",
            schema_dir.display(),
            dest.display()
        )));
    }

    run_git(dest, &["add", "."], &format!("git add in {}", dest.display()))?;

    let commit_args = if is_rerun {
        vec!["commit", "--amend", "--no-gpg-sign", "-m", "Initial Specify scaffold"]
    } else {
        vec!["commit", "--no-gpg-sign", "-m", "Initial Specify scaffold"]
    };
    run_git(dest, &commit_args, &format!("git commit in {}", dest.display()))?;

    Ok(())
}

/// Resolve the schema cache directory from the initiating repo's `.specify/.cache/`.
/// For a bare identifier like `omnia@v1`, the path is `<initiating>/.specify/.cache/omnia@v1/`.
/// For a URL-shaped identifier, use the last non-empty path segment before any `@ref`.
fn locate_schema_cache(
    schema: &str, initiating_project_dir: &Path,
) -> Result<std::path::PathBuf, Error> {
    let cache_base = initiating_project_dir.join(".specify").join(".cache");

    let direct = cache_base.join(schema);
    if direct.is_dir() {
        return Ok(direct);
    }

    // For URL-shaped identifiers, try the last path segment
    // (strip any @ref suffix, then take the last non-empty segment)
    let without_ref = schema.split('@').next().unwrap_or(schema);
    if let Some(segment) = without_ref.rsplit('/').find(|s| !s.is_empty()) {
        let by_segment = cache_base.join(segment);
        if by_segment.is_dir() {
            return Ok(by_segment);
        }
    }

    Err(Error::Config(format!(
        "schema '{}' not cached in {}; run /spec:init in the initiating repo first",
        schema,
        cache_base.display()
    )))
}

fn run_git(cwd: &Path, args: &[&str], label: &str) -> Result<(), Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("{label}: failed to spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Config(format!("{label} failed: {stderr}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// workspace push (RFC-3b Change 8)
// ---------------------------------------------------------------------------

/// Result of a per-project push operation.
pub struct WorkspacePushResult {
    pub name: String,
    pub status: String,
    pub branch: Option<String>,
    pub pr_number: Option<u64>,
    pub error: Option<String>,
}

/// Extract a GitHub `org/repo` slug from a git remote URL.
/// Returns `None` for non-GitHub URLs.
pub fn extract_github_slug(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let slug = rest.strip_suffix(".git").unwrap_or(rest);
        return Some(slug.to_string());
    }
    for prefix in &["https://github.com/", "http://github.com/"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            let slug = rest.strip_suffix(".git").unwrap_or(rest);
            return Some(slug.to_string());
        }
    }
    if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        let slug = rest.strip_suffix(".git").unwrap_or(rest);
        return Some(slug.to_string());
    }
    None
}

/// Core implementation of `specify workspace push`.
pub fn run_workspace_push_impl(
    project_dir: &Path, plan: &Plan, registry: &Registry, filter_projects: &[String], dry_run: bool,
) -> Result<Vec<WorkspacePushResult>, Error> {
    let initiative_name = &plan.name;
    let branch_name = format!("specify/{initiative_name}");
    let workspace_base = ProjectConfig::specify_dir(project_dir).join("workspace");

    let target_projects: Vec<&specify_schema::RegistryProject> = if filter_projects.is_empty() {
        registry.projects.iter().collect()
    } else {
        registry.projects.iter().filter(|p| filter_projects.contains(&p.name)).collect()
    };

    let mut results = Vec::new();

    for rp in &target_projects {
        let result = push_single_project(
            project_dir,
            &workspace_base,
            rp,
            &branch_name,
            initiative_name,
            dry_run,
        );
        results.push(result);
    }

    Ok(results)
}

fn push_single_project(
    project_dir: &Path, workspace_base: &Path, rp: &specify_schema::RegistryProject,
    branch_name: &str, initiative_name: &str, dry_run: bool,
) -> WorkspacePushResult {
    let project_path = if rp.url_materialises_as_symlink() {
        if rp.url == "." { project_dir.to_path_buf() } else { project_dir.join(&rp.url) }
    } else {
        workspace_base.join(&rp.name)
    };

    if !project_path.join(".git").exists() {
        return WorkspacePushResult {
            name: rp.name.clone(),
            status: "failed".to_string(),
            branch: None,
            pr_number: None,
            error: Some(format!("no .git/ found at {}", project_path.display())),
        };
    }

    let remote_url = if rp.url_materialises_as_symlink() {
        match git_output_ok(&project_path, &["remote", "get-url", "origin"]) {
            Some(url) => url,
            None => {
                return WorkspacePushResult {
                    name: rp.name.clone(),
                    status: "local-only".to_string(),
                    branch: None,
                    pr_number: None,
                    error: None,
                };
            }
        }
    } else {
        rp.url.clone()
    };

    let has_commits = git_output_ok(&project_path, &["log", "--oneline", "-1"]).is_some();
    if !has_commits {
        return WorkspacePushResult {
            name: rp.name.clone(),
            status: "up-to-date".to_string(),
            branch: None,
            pr_number: None,
            error: None,
        };
    }

    if dry_run {
        return WorkspacePushResult {
            name: rp.name.clone(),
            status: "pushed".to_string(),
            branch: Some(branch_name.to_string()),
            pr_number: None,
            error: None,
        };
    }

    let branch_result = run_git(
        &project_path,
        &["checkout", "-B", branch_name],
        &format!("checkout -B {branch_name} in {}", rp.name),
    );
    if let Err(e) = branch_result {
        return WorkspacePushResult {
            name: rp.name.clone(),
            status: "failed".to_string(),
            branch: None,
            pr_number: None,
            error: Some(e.to_string()),
        };
    }

    let slug = extract_github_slug(&remote_url);
    let mut is_created = false;

    if let Some(ref slug) = slug {
        let repo_check = Command::new("gh").args(["repo", "view", slug, "--json", "name"]).output();

        match repo_check {
            Ok(output) if !output.status.success() => {
                let create_result = Command::new("gh")
                    .args(["repo", "create", slug, "--private", "--source", "."])
                    .current_dir(&project_path)
                    .output();
                match create_result {
                    Ok(output) if output.status.success() => {
                        is_created = true;
                    }
                    _ => {
                        return WorkspacePushResult {
                            name: rp.name.clone(),
                            status: "failed".to_string(),
                            branch: Some(branch_name.to_string()),
                            pr_number: None,
                            error: Some("failed to create remote repo via gh".to_string()),
                        };
                    }
                }
            }
            _ => {}
        }
    }

    let push_result = run_git(
        &project_path,
        &["push", "--force-with-lease", "-u", "origin", branch_name],
        &format!("git push in {}", rp.name),
    );
    if let Err(e) = push_result {
        return WorkspacePushResult {
            name: rp.name.clone(),
            status: "failed".to_string(),
            branch: Some(branch_name.to_string()),
            pr_number: None,
            error: Some(e.to_string()),
        };
    }

    let mut pr_number = None;
    if let Some(ref _slug) = slug {
        let pr_check = Command::new("gh")
            .args(["pr", "list", "--head", branch_name, "--json", "number", "--limit", "1"])
            .current_dir(&project_path)
            .output();

        if let Ok(output) = pr_check
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(&text)
                && let Some(first) = parsed.first()
            {
                pr_number = first.get("number").and_then(|n| n.as_u64());
            }
        }

        if pr_number.is_none() {
            let pr_title = format!("specify: {initiative_name}");
            let pr_body = format!(
                "Automated push from specify workspace push for initiative \
                 `{initiative_name}`."
            );
            let pr_create = Command::new("gh")
                .args(["pr", "create", "--title", &pr_title, "--body", &pr_body])
                .current_dir(&project_path)
                .output();

            if let Ok(output) = pr_create
                && output.status.success()
            {
                let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(num_str) = url.rsplit('/').next() {
                    pr_number = num_str.parse().ok();
                }
            }
        }
    }

    let status = if is_created { "created" } else { "pushed" };

    WorkspacePushResult {
        name: rp.name.clone(),
        status: status.to_string(),
        branch: Some(branch_name.to_string()),
        pr_number,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_github_slug_git_ssh() {
        assert_eq!(
            extract_github_slug("git@github.com:org/mobile.git"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_git_ssh_no_suffix() {
        assert_eq!(
            extract_github_slug("git@github.com:org/mobile"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_https() {
        assert_eq!(
            extract_github_slug("https://github.com/org/mobile.git"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_https_no_suffix() {
        assert_eq!(
            extract_github_slug("https://github.com/org/mobile"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_ssh_protocol() {
        assert_eq!(
            extract_github_slug("ssh://git@github.com/org/mobile.git"),
            Some("org/mobile".to_string())
        );
    }

    #[test]
    fn extract_github_slug_non_github() {
        assert_eq!(extract_github_slug("git@gitlab.com:org/repo.git"), None);
    }
}
