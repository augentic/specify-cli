//! `slice merge run | preview | conflict-check`.
//!
//! Owns the merge-side JSON DTOs, summarisers, and the workspace-clone
//! auto-commit shim that runs after a merge inside a workspace clone.

use std::io::Write;
use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use specify_config::{LayoutExt, is_workspace_clone_path};
use specify_error::Result;
use specify_merge::{
    BaselineConflict, MergePreviewEntry, OpaqueAction, OpaquePreviewEntry, conflict_check, slice,
};

use super::artifact_classes;
use crate::context::Ctx;
use crate::output::Render;

const WORKSPACE_MERGE_COMMIT_PATHS: [&str; 2] = [".specify/specs", ".specify/archive"];

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let archive_dir = ctx.archive_dir();
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);

    let merged = slice::commit(&slice_dir, &classes, &archive_dir, Utc::now())?;

    // The merge-owned workspace commit is limited to the baseline spec
    // tree and archived slice. Opaque/generated outputs remain as residue
    // for the execute driver.
    if is_workspace_clone(&ctx.project_dir) {
        auto_commit(&ctx.project_dir, name);
    }

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let archive_path = archive_dir.join(format!("{today}-{name}"));

    let entries: Vec<MergedEntry> = merged.iter().map(MergedEntry::from).collect();
    ctx.out().write(&MergeRunBody {
        merged_specs: entries,
        archive_path: archive_path.display().to_string(),
    })?;
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
    let specs: Vec<SpecPreviewEntry> = result
        .three_way
        .iter()
        .filter(|e| e.class_name == "specs")
        .map(SpecPreviewEntry::from)
        .collect();
    let contracts: Vec<ContractItem> = result
        .opaque
        .iter()
        .filter(|e| e.class_name == "contracts")
        .map(ContractItem::from)
        .collect();

    ctx.out().write(&PreviewBody {
        slice_dir: slice_dir.display().to_string(),
        specs,
        contracts,
    })?;
    Ok(())
}

pub(super) fn conflicts(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);
    let conflicts = conflict_check(&slice_dir, &classes)?;
    let rows: Vec<ConflictRow> = conflicts.iter().map(ConflictRow::from).collect();

    ctx.out().write(&ConflictCheckBody {
        slice_dir: slice_dir.display().to_string(),
        conflicts: rows,
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Bodies + rows.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct MergeRunBody {
    merged_specs: Vec<MergedEntry>,
    #[serde(skip)]
    archive_path: String,
}

impl Render for MergeRunBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        for entry in &self.merged_specs {
            writeln!(w, "{}: {}", entry.name, summarise_ops(&entry.operations))?;
        }
        writeln!(w, "Archived to {}", self.archive_path)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct MergedEntry {
    name: String,
    operations: Vec<MergeOpJson>,
}

impl From<&MergePreviewEntry> for MergedEntry {
    fn from(entry: &MergePreviewEntry) -> Self {
        Self {
            name: entry.name.clone(),
            operations: entry.result.operations.iter().map(MergeOpJson::from).collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PreviewBody {
    slice_dir: String,
    specs: Vec<SpecPreviewEntry>,
    contracts: Vec<ContractItem>,
}

impl Render for PreviewBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.specs.is_empty() {
            writeln!(w, "No delta specs to merge.")?;
        } else {
            for entry in &self.specs {
                writeln!(w, "{}: {}", entry.name, summarise_ops(&entry.operations))?;
                for op in &entry.operations {
                    writeln!(w, "  {}", operation_label(op))?;
                }
            }
        }
        if !self.contracts.is_empty() {
            writeln!(w, "\nContract changes:")?;
            for c in &self.contracts {
                let (sigil, label) = match c.action {
                    ContractAction::Added => ("+", "added"),
                    ContractAction::Replaced => ("~", "replaced"),
                    ContractAction::Unknown => ("?", "unknown"),
                };
                writeln!(w, "  {sigil} contracts/{} ({label})", c.path)?;
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SpecPreviewEntry {
    name: String,
    baseline_path: String,
    operations: Vec<MergeOpJson>,
}

impl From<&MergePreviewEntry> for SpecPreviewEntry {
    fn from(entry: &MergePreviewEntry) -> Self {
        Self {
            name: entry.name.clone(),
            baseline_path: entry.baseline_path.display().to_string(),
            operations: entry.result.operations.iter().map(MergeOpJson::from).collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContractItem {
    path: String,
    action: ContractAction,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum ContractAction {
    Added,
    Replaced,
    Unknown,
}

impl From<&OpaquePreviewEntry> for ContractItem {
    fn from(entry: &OpaquePreviewEntry) -> Self {
        let action = match entry.action {
            OpaqueAction::Added => ContractAction::Added,
            OpaqueAction::Replaced => ContractAction::Replaced,
            _ => ContractAction::Unknown,
        };
        Self {
            path: entry.relative_path.clone(),
            action,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConflictCheckBody {
    slice_dir: String,
    conflicts: Vec<ConflictRow>,
}

impl Render for ConflictCheckBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.conflicts.is_empty() {
            return writeln!(w, "No baseline conflicts.");
        }
        for c in &self.conflicts {
            writeln!(
                w,
                "{}: baseline modified {} (defined_at {})",
                c.capability, c.baseline_modified_at, c.defined_at,
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConflictRow {
    capability: String,
    defined_at: String,
    baseline_modified_at: String,
}

impl From<&BaselineConflict> for ConflictRow {
    fn from(c: &BaselineConflict) -> Self {
        Self {
            capability: c.capability.clone(),
            defined_at: c.defined_at.clone(),
            baseline_modified_at: c.baseline_modified_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// MergeOpJson — typed wire representation of a `MergeOperation`.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum MergeOpJson {
    Added { id: String, name: String },
    Modified { id: String, name: String },
    Removed { id: String, name: String },
    Renamed { id: String, old_name: String, new_name: String },
    CreatedBaseline { requirement_count: usize },
    Unknown,
}

impl From<&specify_merge::MergeOperation> for MergeOpJson {
    fn from(op: &specify_merge::MergeOperation) -> Self {
        use specify_merge::MergeOperation;
        match op {
            MergeOperation::Added { id, name } => Self::Added {
                id: id.clone(),
                name: name.clone(),
            },
            MergeOperation::Modified { id, name } => Self::Modified {
                id: id.clone(),
                name: name.clone(),
            },
            MergeOperation::Removed { id, name } => Self::Removed {
                id: id.clone(),
                name: name.clone(),
            },
            MergeOperation::Renamed {
                id,
                old_name,
                new_name,
            } => Self::Renamed {
                id: id.clone(),
                old_name: old_name.clone(),
                new_name: new_name.clone(),
            },
            MergeOperation::CreatedBaseline { requirement_count } => Self::CreatedBaseline {
                requirement_count: *requirement_count,
            },
            // `MergeOperation` is `#[non_exhaustive]`; future variants
            // surface as `{"kind": "unknown"}` until mapped.
            _ => Self::Unknown,
        }
    }
}

fn operation_label(op: &MergeOpJson) -> String {
    match op {
        MergeOpJson::Added { id, name } => format!("ADDING: {id} — {name}"),
        MergeOpJson::Modified { id, name } => format!("MODIFYING: {id} — {name}"),
        MergeOpJson::Removed { id, name } => format!("REMOVING: {id} — {name}"),
        MergeOpJson::Renamed {
            id,
            old_name,
            new_name,
        } => format!("RENAMING: {id} — {old_name} -> {new_name}"),
        MergeOpJson::CreatedBaseline { requirement_count } => {
            format!("CREATING baseline with {requirement_count} requirement(s)")
        }
        MergeOpJson::Unknown => "UNKNOWN operation".to_string(),
    }
}

fn summarise_ops(ops: &[MergeOpJson]) -> String {
    let mut added = 0;
    let mut modified = 0;
    let mut removed = 0;
    let mut renamed = 0;
    let mut created_baseline = None;
    for op in ops {
        match op {
            MergeOpJson::Added { .. } => added += 1,
            MergeOpJson::Modified { .. } => modified += 1,
            MergeOpJson::Removed { .. } => removed += 1,
            MergeOpJson::Renamed { .. } => renamed += 1,
            MergeOpJson::CreatedBaseline { requirement_count } => {
                created_baseline = Some(*requirement_count);
            }
            MergeOpJson::Unknown => {}
        }
    }
    if let Some(count) = created_baseline {
        return format!("created baseline with {count} requirement(s)");
    }
    let mut parts: Vec<String> = Vec::new();
    if added > 0 {
        parts.push(format!("+{added} added"));
    }
    if modified > 0 {
        parts.push(format!("{modified} modified"));
    }
    if removed > 0 {
        parts.push(format!("-{removed} removed"));
    }
    if renamed > 0 {
        parts.push(format!("{renamed} renamed"));
    }
    if parts.is_empty() { "no-op".to_string() } else { parts.join(", ") }
}

// ---------------------------------------------------------------------------
// Workspace-clone auto-commit.
// ---------------------------------------------------------------------------

/// Detect whether a project directory is inside a workspace clone.
/// Two-part heuristic: (1) the path contains `/.specify/workspace/*/`
/// as an ancestor via structural component walk, and (2)
/// `.specify/project.yaml` exists in the project directory. The
/// secondary guard — CWD does not contain `.specify/plan.yaml` — is
/// retained as a safety check but is not sufficient on its own
/// because `plan.yaml` may be absent after `specify change plan
/// archive`.
fn is_workspace_clone(project_dir: &Path) -> bool {
    if !is_workspace_clone_path(project_dir) {
        return false;
    }
    let has_project_yaml = project_dir.join(".specify").join("project.yaml").exists();
    let has_plan_yaml = project_dir.layout().plan_path().exists();
    has_project_yaml && !has_plan_yaml
}

fn merge_pathspecs(project_dir: &Path) -> Vec<&'static str> {
    WORKSPACE_MERGE_COMMIT_PATHS
        .iter()
        .copied()
        .filter(|path| project_dir.join(path).exists())
        .collect()
}

fn git(project_dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("git").arg("-C").arg(project_dir).args(args).output()
}

fn auto_commit(project_dir: &Path, name: &str) {
    let pathspecs = merge_pathspecs(project_dir);
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
        assert!(is_workspace_clone(&path));
    }

    #[test]
    fn rejects_normal_project_root() {
        let path = Path::new("/home/user/project/");
        assert!(!is_workspace_clone(path));
    }

    #[test]
    fn rejects_bare_specify_dir() {
        let path = Path::new("/home/user/project/.specify/");
        assert!(!is_workspace_clone(path));
    }

    #[test]
    fn deeply_nested_workspace_clone() {
        let tmp = workspace_clone_dir("mobile");
        let path =
            tmp.path().join(".specify").join("workspace").join("mobile").join("sub").join("dir");
        std::fs::create_dir_all(path.join(".specify")).unwrap();
        std::fs::write(path.join(".specify").join("project.yaml"), "name: stub\n").unwrap();
        assert!(is_workspace_clone(&path));
    }
}
