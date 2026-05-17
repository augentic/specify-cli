//! `slice merge run | preview | conflict-check`. Owns the merge-side
//! JSON DTOs, summarisers, and the workspace-clone auto-commit shim.

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::config::{Layout, is_workspace_clone};
use specify_domain::merge::{
    BaselineConflict, MergeOperation, MergePreviewEntry, OpaqueAction, conflict_check, slice,
};
use specify_error::Result;

use super::artifact_classes;
use crate::context::Ctx;

const WORKSPACE_MERGE_COMMIT_PATHS: [&str; 2] = [".specify/specs", ".specify/archive"];

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let archive_dir = ctx.archive_dir();
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);

    let merged = slice::commit(&slice_dir, &classes, &archive_dir, Timestamp::now())?;

    // The merge-owned workspace commit is limited to the baseline spec
    // tree and archived slice. Opaque/generated outputs remain as residue
    // for the execute driver.
    if is_clone_eligible(&ctx.project_dir) {
        auto_commit(&ctx.project_dir, name);
    }

    let today = Timestamp::now().strftime("%Y-%m-%d").to_string();
    let archive_path = archive_dir.join(format!("{today}-{name}"));

    ctx.write(
        &RunBody {
            merged_specs: &merged,
            archive_path,
        },
        write_run_text,
    )?;
    Ok(())
}

pub(super) fn preview(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);
    let result = slice::preview(&slice_dir, &classes)?;

    // The JSON preview surface keeps its `specs` and `contracts` arrays
    // by grouping the engine's class-tagged entries by their `class_name`.
    // The literal output keys live here — alongside the omnia-default
    // synthesiser — rather than in the engine.
    let specs: Vec<&MergePreviewEntry> =
        result.three_way.iter().filter(|e| e.class_name == "specs").collect();
    let contracts: Vec<ContractItem> = result
        .opaque
        .iter()
        .filter(|e| e.class_name == "contracts")
        .map(|entry| ContractItem {
            path: entry.relative_path.clone(),
            action: entry.action,
        })
        .collect();

    ctx.write(
        &PreviewBody {
            slice_dir: slice_dir.display().to_string(),
            specs,
            contracts,
        },
        write_preview_text,
    )?;
    Ok(())
}

pub(super) fn conflicts(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);
    let conflicts = conflict_check(&slice_dir, &classes)?;

    ctx.write(
        &ConflictCheckBody {
            slice_dir: slice_dir.display().to_string(),
            conflicts: &conflicts,
        },
        write_conflict_check_text,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Bodies.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct RunBody<'a> {
    merged_specs: &'a [MergePreviewEntry],
    #[serde(skip)]
    archive_path: PathBuf,
}

fn write_run_text(w: &mut dyn Write, body: &RunBody<'_>) -> std::io::Result<()> {
    for entry in body.merged_specs {
        writeln!(w, "{}: {}", entry.name, summarise_ops(&entry.result.operations))?;
    }
    writeln!(w, "Archived to {}", body.archive_path.display())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PreviewBody<'a> {
    slice_dir: String,
    specs: Vec<&'a MergePreviewEntry>,
    contracts: Vec<ContractItem>,
}

fn write_preview_text(w: &mut dyn Write, body: &PreviewBody<'_>) -> std::io::Result<()> {
    if body.specs.is_empty() {
        writeln!(w, "No delta specs to merge.")?;
    } else {
        for entry in &body.specs {
            writeln!(w, "{}: {}", entry.name, summarise_ops(&entry.result.operations))?;
            for op in &entry.result.operations {
                writeln!(w, "  {}", operation_label(op))?;
            }
        }
    }
    if !body.contracts.is_empty() {
        writeln!(w, "\nContract changes:")?;
        for c in &body.contracts {
            let (sigil, label) = match c.action {
                OpaqueAction::Added => ("+", "added"),
                OpaqueAction::Replaced => ("~", "replaced"),
            };
            writeln!(w, "  {sigil} contracts/{} ({label})", c.path)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContractItem {
    path: String,
    action: OpaqueAction,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConflictCheckBody<'a> {
    slice_dir: String,
    conflicts: &'a [BaselineConflict],
}

fn write_conflict_check_text(
    w: &mut dyn Write, body: &ConflictCheckBody<'_>,
) -> std::io::Result<()> {
    if body.conflicts.is_empty() {
        return writeln!(w, "No baseline conflicts.");
    }
    for c in body.conflicts {
        writeln!(
            w,
            "{}: baseline modified {} (defined_at {})",
            c.capability,
            c.baseline_modified_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
            c.defined_at,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MergeOperation rendering.
// ---------------------------------------------------------------------------

fn operation_label(op: &MergeOperation) -> String {
    match op {
        MergeOperation::Added { id, name } => format!("ADDING: {id} — {name}"),
        MergeOperation::Modified { id, name } => format!("MODIFYING: {id} — {name}"),
        MergeOperation::Removed { id, name } => format!("REMOVING: {id} — {name}"),
        MergeOperation::Renamed {
            id,
            old_name,
            new_name,
        } => format!("RENAMING: {id} — {old_name} -> {new_name}"),
        MergeOperation::CreatedBaseline { requirement_count } => {
            format!("CREATING baseline with {requirement_count} requirement(s)")
        }
    }
}

fn summarise_ops(ops: &[MergeOperation]) -> String {
    let mut counts: [(u32, &str, &str); 4] =
        [(0, "added", "+"), (0, "modified", ""), (0, "removed", "-"), (0, "renamed", "")];
    let mut created_baseline = None;
    for op in ops {
        match op {
            MergeOperation::Added { .. } => counts[0].0 += 1,
            MergeOperation::Modified { .. } => counts[1].0 += 1,
            MergeOperation::Removed { .. } => counts[2].0 += 1,
            MergeOperation::Renamed { .. } => counts[3].0 += 1,
            MergeOperation::CreatedBaseline { requirement_count } => {
                created_baseline = Some(*requirement_count);
            }
        }
    }
    if let Some(count) = created_baseline {
        return format!("created baseline with {count} requirement(s)");
    }
    let parts: Vec<String> = counts
        .iter()
        .filter(|(c, _, _)| *c > 0)
        .map(|(c, label, prefix)| format!("{prefix}{c} {label}"))
        .collect();
    if parts.is_empty() { "no-op".to_string() } else { parts.join(", ") }
}

// ---------------------------------------------------------------------------
// Workspace-clone auto-commit.
// ---------------------------------------------------------------------------

/// Detect whether a project directory is inside a workspace clone.
/// The path must contain `/.specify/workspace/*/` as an ancestor via
/// structural component walk, and `.specify/plan.yaml` must be absent
/// — the plan file's presence indicates an in-flight change rather
/// than a freshly merged clone. The `.specify/project.yaml` check is
/// already enforced upstream by `Ctx::load`.
fn is_clone_eligible(project_dir: &Path) -> bool {
    if !is_workspace_clone(project_dir) {
        return false;
    }
    !Layout::new(project_dir).plan_path().exists()
}

fn git(project_dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("git").arg("-C").arg(project_dir).args(args).output()
}

fn auto_commit(project_dir: &Path, name: &str) {
    let pathspecs: Vec<&'static str> = WORKSPACE_MERGE_COMMIT_PATHS
        .iter()
        .copied()
        .filter(|path| project_dir.join(path).exists())
        .collect();
    if pathspecs.is_empty() {
        return;
    }
    let warn = |step: &str, msg: &str| eprintln!("warning: workspace auto-commit {step}: {msg}");
    let run = |step: &str, args: &[&str]| -> Option<std::process::Output> {
        match git(project_dir, args) {
            Ok(output) => Some(output),
            Err(err) => {
                warn(step, &err.to_string());
                None
            }
        }
    };

    let mut add_args = vec!["add", "--"];
    add_args.extend(pathspecs.iter().copied());
    let Some(add) = run("git-add", &add_args) else { return };
    if !add.status.success() {
        warn("git-add", &String::from_utf8_lossy(&add.stderr));
        return;
    }

    let mut diff_args = vec!["diff", "--cached", "--quiet", "--"];
    diff_args.extend(pathspecs.iter().copied());
    match git(project_dir, &diff_args).map(|o| o.status) {
        Ok(status) if status.success() => return,
        Ok(status) if status.code() == Some(1) => {}
        Ok(status) => return warn("diff check", &format!("status {status}")),
        Err(err) => return warn("diff check", &err.to_string()),
    }

    let commit_msg = format!("specify: merge {name}");
    let mut commit_args = vec!["commit", "-m", &commit_msg, "--"];
    commit_args.extend(pathspecs.iter().copied());
    if let Some(commit) = run("commit", &commit_args)
        && !commit.status.success()
    {
        warn("commit", &String::from_utf8_lossy(&commit.stderr));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn workspace_clone_dir(suffix: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let slot = tmp.path().join(".specify").join("workspace").join(suffix);
        std::fs::create_dir_all(slot.join(".specify")).unwrap();
        std::fs::write(slot.join(".specify").join("project.yaml"), "name: stub\n").unwrap();
        tmp
    }

    #[test]
    fn workspace_clone_path() {
        let tmp = workspace_clone_dir("traffic");
        let path = tmp.path().join(".specify").join("workspace").join("traffic");
        assert!(is_clone_eligible(&path));
    }

    #[test]
    fn rejects_normal_project_root() {
        let path = Path::new("/home/user/project/");
        assert!(!is_clone_eligible(path));
    }

    #[test]
    fn rejects_bare_specify_dir() {
        let path = Path::new("/home/user/project/.specify/");
        assert!(!is_clone_eligible(path));
    }

    #[test]
    fn deeply_nested_workspace_clone() {
        let tmp = workspace_clone_dir("mobile");
        let path =
            tmp.path().join(".specify").join("workspace").join("mobile").join("sub").join("dir");
        std::fs::create_dir_all(path.join(".specify")).unwrap();
        std::fs::write(path.join(".specify").join("project.yaml"), "name: stub\n").unwrap();
        assert!(is_clone_eligible(&path));
    }
}
