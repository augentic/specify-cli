//! Deterministic workspace branch preparation for RFC-14 `/change:execute`.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use specify_error::is_kebab;

use crate::registry::RegistryProject;

const ORIGIN_HEAD_UNRESOLVED: &str = "origin-head-unresolved";

/// Inputs that define the branch-preparation dirtiness boundary.
#[derive(Debug, Clone)]
pub struct BranchPreparationRequest {
    /// Kebab-case umbrella change name. The target branch is exactly
    /// `specify/<change_name>`.
    pub change_name: String,
    /// Active plan-entry source paths that belong to this slice.
    pub source_paths: Vec<PathBuf>,
    /// Capability-owned output paths that belong to this slice.
    pub output_paths: Vec<PathBuf>,
}

/// Successful branch-preparation result for one workspace slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BranchPreparation {
    /// Registry project name.
    pub project: String,
    /// Prepared worktree path.
    pub slot_path: String,
    /// Target branch, always `specify/<change-name>`.
    pub branch: String,
    /// Remote-default symbolic ref used as the branch base.
    pub base_ref: String,
    /// Commit SHA of `origin/HEAD` after fetch/default-head resolution.
    pub base_sha: String,
    /// Whether the local branch was created or reused.
    pub local_branch: LocalBranchAction,
    /// What happened with `origin/specify/<change-name>`.
    pub remote_branch: RemoteBranchAction,
    /// Tracked/untracked dirtiness classification observed before checkout.
    pub dirty: DirtyClassification,
}

/// Local branch action taken during preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalBranchAction {
    /// A new local branch was created from `origin/HEAD`.
    Created,
    /// An existing local branch was checked out or already current.
    Reused,
}

/// Remote change-branch action taken during preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteBranchAction {
    /// No `origin/specify/<change-name>` exists.
    Absent,
    /// The local branch already matched the remote branch.
    UpToDate,
    /// The local branch fast-forwarded to the remote branch.
    FastForwarded,
    /// The local branch is ahead of the remote branch.
    LocalAhead,
}

/// Dirty-state classification used by branch preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DirtyClassification {
    /// Tracked dirty paths that are safe for slice resume.
    pub tracked_allowed: Vec<String>,
    /// Tracked dirty paths outside the active slice boundary.
    pub tracked_blocked: Vec<String>,
    /// Untracked paths. These do not block branch preparation but remain
    /// visible so push/status can refuse dirty checkouts later.
    pub untracked: Vec<String>,
    /// Relative path prefixes used for the allowed tracked classification.
    pub allowed_paths: Vec<String>,
}

impl DirtyClassification {
    #[must_use]
    const fn has_allowed_tracked(&self) -> bool {
        !self.tracked_allowed.is_empty()
    }
}

/// Machine-readable failure emitted before any unsafe branch mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BranchPreparationDiagnostic {
    /// Stable diagnostic key.
    pub key: String,
    /// Registry project name.
    pub project: String,
    /// Human-readable diagnostic.
    pub message: String,
    /// Target branch when it could be derived safely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Relevant paths for dirty-path diagnostics.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl BranchPreparationDiagnostic {
    #[must_use]
    fn new(
        key: impl Into<String>, project: &RegistryProject, branch: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            project: project.name.clone(),
            message: message.into(),
            branch: branch.map(ToString::to_string),
            paths: Vec::new(),
        }
    }

    #[must_use]
    fn with_paths(mut self, paths: Vec<String>) -> Self {
        self.paths = paths;
        self
    }
}

/// Prepare a resolved registry project's worktree on `specify/<change-name>`.
///
/// # Errors
///
/// Returns a structured diagnostic when the slot is missing, the remote default
/// cannot be resolved, the branch name is outside the RFC-14 pattern, unrelated
/// tracked work is dirty, or a required Git operation fails.
pub fn prepare_project_branch(
    project_dir: &Path, project: &RegistryProject, request: &BranchPreparationRequest,
) -> Result<BranchPreparation, BranchPreparationDiagnostic> {
    let branch = target_branch(project, &request.change_name)?;
    let slot_path = project_worktree_path(project_dir, project);
    require_git_worktree(&slot_path, project, &branch)?;
    let remote_url = require_origin(&slot_path, project, &branch)?;
    if !project.url_materialises_as_symlink() && remote_url != project.url {
        return Err(BranchPreparationDiagnostic::new(
            "origin-mismatch",
            project,
            Some(&branch),
            format!(
                "`{}` origin remote is `{remote_url}`, but registry url is `{}`",
                slot_path.display(),
                project.url
            ),
        ));
    }

    run_git(&slot_path, ["fetch", "origin"], project, Some(&branch), "git fetch origin")?;
    refresh_origin_head(&slot_path);
    let base_ref = resolve_origin_head(&slot_path, project, &branch)?;
    let base_sha = git_output(&slot_path, ["rev-parse", "origin/HEAD"], project, Some(&branch))?;

    let current_branch =
        git_output_optional(&slot_path, ["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let local_exists = git_success(
        &slot_path,
        ["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")],
    );
    let dirty = classify_dirty(
        &slot_path,
        &request.change_name,
        &request.source_paths,
        &request.output_paths,
    );

    if !dirty.tracked_blocked.is_empty() {
        return Err(BranchPreparationDiagnostic::new(
            "dirty-unrelated-tracked",
            project,
            Some(&branch),
            "tracked work outside the active slice boundary blocks branch preparation",
        )
        .with_paths(dirty.tracked_blocked));
    }

    if dirty.has_allowed_tracked() && current_branch.as_deref() != Some(branch.as_str()) {
        return Err(BranchPreparationDiagnostic::new(
            "dirty-branch-mismatch",
            project,
            Some(&branch),
            "resume-safe tracked work is allowed only when already on the change branch",
        )
        .with_paths(dirty.tracked_allowed));
    }

    let local_branch = if local_exists {
        if current_branch.as_deref() != Some(branch.as_str()) {
            run_git(
                &slot_path,
                ["checkout", &branch],
                project,
                Some(&branch),
                &format!("git checkout {branch}"),
            )?;
        }
        LocalBranchAction::Reused
    } else {
        run_git(
            &slot_path,
            ["checkout", "-b", &branch, "origin/HEAD"],
            project,
            Some(&branch),
            &format!("git checkout -b {branch} origin/HEAD"),
        )?;
        LocalBranchAction::Created
    };

    let remote_branch = fast_forward_remote_branch(&slot_path, project, &branch)?;

    Ok(BranchPreparation {
        project: project.name.clone(),
        slot_path: slot_path.to_string_lossy().into_owned(),
        branch,
        base_ref,
        base_sha,
        local_branch,
        remote_branch,
        dirty,
    })
}

fn target_branch(
    project: &RegistryProject, change_name: &str,
) -> Result<String, BranchPreparationDiagnostic> {
    let branch = format!("specify/{change_name}");
    if is_kebab(change_name) && !change_name.contains('/') {
        return Ok(branch);
    }
    Err(BranchPreparationDiagnostic::new(
        "branch-pattern-mismatch",
        project,
        Some(&branch),
        format!(
            "branch `{branch}` is outside the exact `specify/<change-name>` pattern; \
             change names must be kebab-case without slashes"
        ),
    ))
}

fn project_worktree_path(project_dir: &Path, project: &RegistryProject) -> PathBuf {
    let workspace_slot = project_dir.join(".specify").join("workspace").join(&project.name);
    if !project.url_materialises_as_symlink() || workspace_slot.exists() {
        return workspace_slot;
    }
    if project.url == "." { project_dir.to_path_buf() } else { project_dir.join(&project.url) }
}

fn require_git_worktree(
    slot_path: &Path, project: &RegistryProject, branch: &str,
) -> Result<(), BranchPreparationDiagnostic> {
    if !slot_path.exists() {
        return Err(BranchPreparationDiagnostic::new(
            "workspace-slot-missing",
            project,
            Some(branch),
            format!(
                "`{}` does not exist; run `specify workspace sync {}` first",
                slot_path.display(),
                project.name
            ),
        ));
    }
    if !slot_path.join(".git").exists() {
        return Err(BranchPreparationDiagnostic::new(
            "workspace-slot-not-git",
            project,
            Some(branch),
            format!("`{}` is not a git worktree", slot_path.display()),
        ));
    }
    Ok(())
}

fn require_origin(
    slot_path: &Path, project: &RegistryProject, branch: &str,
) -> Result<String, BranchPreparationDiagnostic> {
    git_output(slot_path, ["remote", "get-url", "origin"], project, Some(branch)).map_err(|_err| {
        BranchPreparationDiagnostic::new(
            "missing-origin",
            project,
            Some(branch),
            format!(
                "`{}` has no origin remote; branch preparation requires a remote default",
                slot_path.display()
            ),
        )
    })
}

fn refresh_origin_head(slot_path: &Path) {
    let _ = Command::new("git")
        .arg("-C")
        .arg(slot_path)
        .args(["remote", "set-head", "origin", "--auto"])
        .output();
}

fn resolve_origin_head(
    slot_path: &Path, project: &RegistryProject, branch: &str,
) -> Result<String, BranchPreparationDiagnostic> {
    let symbolic =
        git_output_optional(slot_path, ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]);
    let Some(base_ref) = symbolic else {
        return Err(BranchPreparationDiagnostic::new(
            ORIGIN_HEAD_UNRESOLVED,
            project,
            Some(branch),
            "origin-head-unresolved: could not resolve `origin/HEAD` after fetch; \
             refusing to guess a default branch",
        ));
    };
    git_output(slot_path, ["rev-parse", "--verify", "origin/HEAD^{commit}"], project, Some(branch))
        .map(|_| base_ref)
        .map_err(|_err| {
            BranchPreparationDiagnostic::new(
                ORIGIN_HEAD_UNRESOLVED,
                project,
                Some(branch),
                "origin-head-unresolved: `origin/HEAD` is not a commit; refusing to guess a default branch",
            )
        })
}

fn fast_forward_remote_branch(
    slot_path: &Path, project: &RegistryProject, branch: &str,
) -> Result<RemoteBranchAction, BranchPreparationDiagnostic> {
    let remote_ref = format!("refs/remotes/origin/{branch}");
    if !git_success(slot_path, ["show-ref", "--verify", "--quiet", &remote_ref]) {
        return Ok(RemoteBranchAction::Absent);
    }

    let local = git_output(slot_path, ["rev-parse", "HEAD"], project, Some(branch))?;
    let remote =
        git_output(slot_path, ["rev-parse", &format!("origin/{branch}")], project, Some(branch))?;
    if local == remote {
        return Ok(RemoteBranchAction::UpToDate);
    }
    if git_success(slot_path, ["merge-base", "--is-ancestor", "HEAD", &format!("origin/{branch}")])
    {
        run_git(
            slot_path,
            ["merge", "--ff-only", &format!("origin/{branch}")],
            project,
            Some(branch),
            &format!("git merge --ff-only origin/{branch}"),
        )?;
        return Ok(RemoteBranchAction::FastForwarded);
    }
    if git_success(slot_path, ["merge-base", "--is-ancestor", &format!("origin/{branch}"), "HEAD"])
    {
        return Ok(RemoteBranchAction::LocalAhead);
    }
    Err(BranchPreparationDiagnostic::new(
        "remote-branch-diverged",
        project,
        Some(branch),
        format!("local `{branch}` and `origin/{branch}` have diverged; reconcile manually"),
    ))
}

fn classify_dirty(
    slot_path: &Path, change_name: &str, source_paths: &[PathBuf], output_paths: &[PathBuf],
) -> DirtyClassification {
    let allowed = allowed_paths(slot_path, change_name, source_paths, output_paths);
    let mut tracked_allowed = Vec::new();
    let mut tracked_blocked = Vec::new();
    let mut untracked = Vec::new();

    for entry in porcelain_entries(slot_path) {
        match entry.kind {
            PorcelainKind::Untracked => untracked.push(entry.path),
            PorcelainKind::Tracked => {
                if allowed.iter().any(|allowed_path| allowed_path.matches(&entry.path)) {
                    tracked_allowed.push(entry.path);
                } else {
                    tracked_blocked.push(entry.path);
                }
            }
        }
    }

    tracked_allowed.sort();
    tracked_allowed.dedup();
    tracked_blocked.sort();
    tracked_blocked.dedup();
    untracked.sort();
    untracked.dedup();

    DirtyClassification {
        tracked_allowed,
        tracked_blocked,
        untracked,
        allowed_paths: allowed.into_iter().map(|path| path.display).collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowedPath {
    rel: String,
    display: String,
    is_dir: bool,
}

impl AllowedPath {
    fn matches(&self, path: &str) -> bool {
        path == self.rel || (self.is_dir && path.starts_with(&format!("{}/", self.rel)))
    }
}

fn allowed_paths(
    slot_path: &Path, change_name: &str, source_paths: &[PathBuf], output_paths: &[PathBuf],
) -> Vec<AllowedPath> {
    let mut allowed = vec![
        AllowedPath {
            rel: format!(".specify/slices/{change_name}"),
            display: format!(".specify/slices/{change_name}/"),
            is_dir: true,
        },
        AllowedPath {
            rel: ".specify/specs".to_string(),
            display: ".specify/specs/".to_string(),
            is_dir: true,
        },
        AllowedPath {
            rel: ".specify/archive".to_string(),
            display: ".specify/archive/".to_string(),
            is_dir: true,
        },
        AllowedPath {
            rel: "crates".to_string(),
            display: "crates/".to_string(),
            is_dir: true,
        },
        AllowedPath {
            rel: "contracts".to_string(),
            display: "contracts/".to_string(),
            is_dir: true,
        },
        AllowedPath {
            rel: "apps".to_string(),
            display: "apps/".to_string(),
            is_dir: true,
        },
    ];

    for path in source_paths.iter().chain(output_paths) {
        if let Some((rel, is_dir)) = relative_allowed_path(slot_path, path) {
            let display = if is_dir { format!("{rel}/") } else { rel.clone() };
            allowed.push(AllowedPath { rel, display, is_dir });
        }
    }

    allowed.sort_by(|a, b| a.rel.cmp(&b.rel));
    allowed.dedup_by(|a, b| a.rel == b.rel && a.is_dir == b.is_dir);
    allowed
}

fn relative_allowed_path(slot_path: &Path, input: &Path) -> Option<(String, bool)> {
    let candidate = if input.is_absolute() {
        let canonical_slot = std::fs::canonicalize(slot_path).ok();
        input
            .strip_prefix(slot_path)
            .ok()
            .or_else(|| canonical_slot.as_deref().and_then(|slot| input.strip_prefix(slot).ok()))?
            .to_path_buf()
    } else {
        input.to_path_buf()
    };
    let rel = path_to_slash(&candidate)?;
    if rel.is_empty() {
        return None;
    }
    let is_dir = slot_path.join(&candidate).is_dir();
    Some((rel, is_dir))
}

fn path_to_slash(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PorcelainEntry {
    kind: PorcelainKind,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PorcelainKind {
    Tracked,
    Untracked,
}

fn porcelain_entries(slot_path: &Path) -> Vec<PorcelainEntry> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(slot_path)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let records: Vec<&[u8]> =
        output.stdout.split(|byte| *byte == 0).filter(|record| !record.is_empty()).collect();
    let mut entries = Vec::new();
    let mut idx = 0;
    while idx < records.len() {
        let record = records[idx];
        if record.len() < 4 {
            idx += 1;
            continue;
        }
        let status = &record[..2];
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        match status {
            b"??" => entries.push(PorcelainEntry {
                kind: PorcelainKind::Untracked,
                path,
            }),
            b"!!" => {}
            _ => {
                entries.push(PorcelainEntry {
                    kind: PorcelainKind::Tracked,
                    path,
                });
                if (matches!(status[0], b'R' | b'C') || matches!(status[1], b'R' | b'C'))
                    && let Some(original) = records.get(idx + 1)
                {
                    entries.push(PorcelainEntry {
                        kind: PorcelainKind::Tracked,
                        path: String::from_utf8_lossy(original).into_owned(),
                    });
                    idx += 1;
                }
            }
        }
        idx += 1;
    }
    entries
}

fn run_git<I, S>(
    cwd: &Path, args: I, project: &RegistryProject, branch: Option<&str>, label: &str,
) -> Result<(), BranchPreparationDiagnostic>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output().map_err(|err| {
        BranchPreparationDiagnostic::new(
            "git-command-failed",
            project,
            branch,
            format!("{label}: failed to spawn git: {err}"),
        )
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(BranchPreparationDiagnostic::new(
        "git-command-failed",
        project,
        branch,
        format!("{label} failed: {}", String::from_utf8_lossy(&output.stderr).trim()),
    ))
}

fn git_output<I, S>(
    cwd: &Path, args: I, project: &RegistryProject, branch: Option<&str>,
) -> Result<String, BranchPreparationDiagnostic>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output().map_err(|err| {
        BranchPreparationDiagnostic::new(
            "git-command-failed",
            project,
            branch,
            format!("failed to spawn git: {err}"),
        )
    })?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            return Ok(text);
        }
    }
    Err(BranchPreparationDiagnostic::new(
        "git-command-failed",
        project,
        branch,
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn git_output_optional<I, S>(cwd: &Path, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn git_success<I, S>(cwd: &Path, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}
