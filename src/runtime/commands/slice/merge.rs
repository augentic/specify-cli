//! `slice merge run | preview | conflict-check`. Owns the merge-side
//! JSON DTOs, summarisers, and the workspace-clone auto-commit shim.

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_workflow::change::{Plan, Status};
use specify_workflow::config::{Layout, is_slot, with_state};
use specify_workflow::journal::{self, Event, EventKind};
use specify_workflow::merge::{
    BaselineConflict, MergeOperation, MergePreviewEntry, OpaqueAction, conflict_check, slice,
};

use super::artifact_classes;
use crate::runtime::context::Ctx;

const WORKSPACE_MERGE_COMMIT_PATHS: [&str; 2] = [".specify/specs", ".specify/archive"];

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    // RFC-29d: the `slice.merge.*` pair fires on the validator outcome.
    // `started` brackets entry; the fallible body runs the validator +
    // apply and (on success) the durable `slice.archive.created` ledger
    // entry; `succeeded` brackets a fully completed run. Ordering is
    // started → … → archive.created → succeeded, so the lifecycle pair
    // wraps the ledger entry rather than racing it.
    emit_merge_event(
        ctx,
        EventKind::SliceMergeStarted {
            slice_name: name.into(),
        },
    );
    match commit_run(ctx, name) {
        Ok(()) => {
            emit_merge_event(
                ctx,
                EventKind::SliceMergeSucceeded {
                    slice_name: name.into(),
                },
            );
            Ok(())
        }
        Err(err) => {
            // `reason` is the error's stable kebab discriminant. The
            // failed event is best-effort like the rest, but the
            // original error still propagates so the exit code is
            // unchanged.
            emit_merge_event(
                ctx,
                EventKind::SliceMergeFailed {
                    slice_name: name.into(),
                    reason: err.variant_str().into_owned(),
                },
            );
            Err(err)
        }
    }
}

/// Validator + apply core of `slice merge run`: commit the deltas,
/// auto-commit the workspace clone, append the outcome-ledger entry,
/// stamp the plan entry `done`, and write the run output. Wrapped by
/// [`run`] so the `slice.merge.*` lifecycle pair can bracket it.
fn commit_run(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let archive_dir = ctx.archive_dir();
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);

    let now = Timestamp::now();
    let merged = slice::commit(&slice_dir, &classes, &archive_dir, now)?;

    // The merge-owned workspace commit is limited to the baseline spec
    // tree and archived slice. Opaque/generated outputs remain as residue
    // for the execute driver.
    if is_clone_eligible(&ctx.project_dir) {
        auto_commit(&ctx.project_dir, name);
    }

    // Append the durable outcome-ledger entry (decision-log §"History
    // via git plus an outcome ledger"). Best-effort: a journal write
    // failure must not undo a committed merge, so the error is logged,
    // not propagated.
    emit_archive_created(ctx, name, &merged, &merged.decisions, now);

    stamp_plan_entry_done(ctx, name)?;

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

/// Best-effort append of a single `slice.merge.*` lifecycle event.
/// Mirrors [`emit_archive_created`]'s posture: a journal-write failure
/// is logged and swallowed so it can never change the merge's exit
/// code. The `failed` variant's emit is equally best-effort, but the
/// caller still propagates the original merge error afterwards.
fn emit_merge_event(ctx: &Ctx, kind: EventKind) {
    let event = Event::new(Timestamp::now(), kind);
    if let Err(err) = journal::append_batch(ctx.layout(), std::slice::from_ref(&event)) {
        eprintln!("warning: slice.merge journal append: {err}");
    }
}

/// Append the `slice.archive.created` outcome-ledger event. Captures
/// the merged baseline spec names, a one-line summary, and the git HEAD
/// SHA after the merge (best-effort). A journal-write or git failure is
/// logged and swallowed — the merge has already committed to disk, so a
/// ledger hiccup must never surface as a non-zero exit.
fn emit_archive_created(
    ctx: &Ctx, name: &str, merged: &[MergePreviewEntry], decisions: &[String], now: Timestamp,
) {
    let touched_specs: Vec<String> = merged.iter().map(|e| e.name.clone()).collect();
    let outcome_summary = if merged.is_empty() {
        "no baseline specs touched".to_string()
    } else {
        merged
            .iter()
            .map(|e| format!("{}: {}", e.name, summarise_ops(&e.result.operations)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let event = Event::new(
        now,
        EventKind::SliceArchiveCreated {
            slice_name: name.into(),
            touched_specs,
            outcome_summary,
            merge_sha: git_head_sha(&ctx.project_dir),
            decisions: decisions.to_vec(),
        },
    );
    if let Err(err) = journal::append_batch(ctx.layout(), std::slice::from_ref(&event)) {
        eprintln!("warning: slice.archive.created journal append: {err}");
    }
}

/// Read the current git HEAD SHA, or `None` when the project is not a
/// git repository or `git` is unavailable.
fn git_head_sha(project_dir: &Path) -> Option<String> {
    let output = git(project_dir, &["rev-parse", "HEAD"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// workflow §Workflow: `/spec:merge` is the sole writer of per-entry
/// `done`. Standalone merge fixtures without `plan.yaml` skip this
/// step silently.
fn stamp_plan_entry_done(ctx: &Ctx, name: &str) -> Result<()> {
    if !ctx.layout().plan_path().exists() {
        return Ok(());
    }
    with_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
        if !plan.entries.iter().any(|e| e.name == name) {
            return Err(Error::Diag {
                code: "plan-entry-not-found",
                detail: format!("no slice named '{name}' in plan"),
            });
        }
        plan.transition(name, Status::Done)?;
        Ok(())
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
            c.adapter,
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
    let mut counts: [(u32, &str); 4] =
        [(0, "added"), (0, "modified"), (0, "removed"), (0, "renamed")];
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
    let parts: Vec<String> =
        counts.iter().filter(|(c, _)| *c > 0).map(|(c, label)| format!("{c} {label}")).collect();
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
    if !is_slot(project_dir) {
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
    let mut add_args = vec!["add", "--"];
    add_args.extend(pathspecs.iter().copied());
    let add = match git(project_dir, &add_args) {
        Ok(output) => output,
        Err(err) => return eprintln!("warning: workspace auto-commit git-add: {err}"),
    };
    if !add.status.success() {
        let stderr = String::from_utf8_lossy(&add.stderr);
        return eprintln!("warning: workspace auto-commit git-add: {stderr}");
    }

    let mut diff_args = vec!["diff", "--cached", "--quiet", "--"];
    diff_args.extend(pathspecs.iter().copied());
    match git(project_dir, &diff_args).map(|o| o.status) {
        Ok(status) if status.success() => return,
        Ok(status) if status.code() == Some(1) => {}
        Ok(status) => {
            eprintln!("warning: workspace auto-commit diff check: status {status}");
            return;
        }
        Err(err) => return eprintln!("warning: workspace auto-commit diff check: {err}"),
    }

    let commit_msg = format!("specify: merge {name}");
    let mut commit_args = vec!["commit", "-m", &commit_msg, "--"];
    commit_args.extend(pathspecs.iter().copied());
    match git(project_dir, &commit_args) {
        Ok(commit) if !commit.status.success() => {
            let stderr = String::from_utf8_lossy(&commit.stderr);
            eprintln!("warning: workspace auto-commit commit: {stderr}");
        }
        Ok(_) => {}
        Err(err) => eprintln!("warning: workspace auto-commit commit: {err}"),
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
