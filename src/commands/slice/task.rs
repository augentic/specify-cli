//! `slice task progress | mark` — task list operations for a slice.
//!
//! Also exposes `resolve_tasks_path_for` so `super::list::collect_status`
//! can read tasks counts for the dashboard.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_capability::{Brief, PipelineView};
use specify_error::{Error, Result};
use specify_slice::SliceMetadata;
use specify_slice::atomic::atomic_bytes_write;
use specify_task::{Task, mark_complete, parse_tasks};

use crate::context::CommandContext;
use crate::output::{Render, emit};

pub(super) fn progress(ctx: &CommandContext, name: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let tasks_path = resolve_tasks_path(&ctx.project_dir, &slice_dir)?;
    let content = std::fs::read_to_string(&tasks_path)?;
    let progress = parse_tasks(&content);

    let tasks: Vec<TaskRow> = progress.tasks.iter().map(TaskRow::from_parsed).collect();
    emit(
        ctx.format,
        &ProgressBody {
            total: progress.total,
            complete: progress.complete,
            pending: progress.total.saturating_sub(progress.complete),
            tasks,
        },
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ProgressBody {
    total: usize,
    complete: usize,
    pending: usize,
    tasks: Vec<TaskRow>,
}

impl Render for ProgressBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{}/{} tasks complete", self.complete, self.total)?;
        for task in &self.tasks {
            let mark = if task.complete { "x" } else { " " };
            writeln!(w, "  [{}] {} {}", mark, task.number, task.description)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TaskRow {
    group: String,
    number: String,
    description: String,
    complete: bool,
    skill_directive: Option<DirectiveRow>,
}

impl TaskRow {
    fn from_parsed(t: &Task) -> Self {
        Self {
            group: t.group.clone(),
            number: t.number.clone(),
            description: t.description.clone(),
            complete: t.complete,
            skill_directive: t.skill_directive.as_ref().map(|d| DirectiveRow {
                plugin: d.plugin.clone(),
                skill: d.skill.clone(),
            }),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DirectiveRow {
    plugin: String,
    skill: String,
}

pub(super) fn mark(ctx: &CommandContext, name: String, task_number: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let tasks_path = resolve_tasks_path(&ctx.project_dir, &slice_dir)?;
    let original = std::fs::read_to_string(&tasks_path)?;
    let updated = mark_complete(&original, &task_number)?;
    let idempotent = updated == original;
    if !idempotent {
        atomic_bytes_write(&tasks_path, updated.as_bytes())?;
    }

    emit(
        ctx.format,
        &MarkBody {
            marked: task_number,
            new_content_path: tasks_path.display().to_string(),
            idempotent,
        },
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct MarkBody {
    marked: String,
    new_content_path: String,
    idempotent: bool,
}

impl Render for MarkBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.idempotent {
            writeln!(w, "Task {} already complete.", self.marked)
        } else {
            writeln!(w, "Marked task {} complete.", self.marked)
        }
    }
}

/// Resolve the `tasks.md` path for a slice.
///
/// Walks the pipeline view to find the `build` brief's `tracks` value
/// (the id of the tasks brief), then uses that brief's `generates`
/// field as the relative path under `slice_dir`. This lets the CLI
/// honour schemas that rename `tasks.md` or nest it elsewhere.
fn resolve_tasks_path(project_dir: &Path, slice_dir: &Path) -> Result<PathBuf> {
    let metadata = SliceMetadata::load(slice_dir)?;
    resolve_tasks_path_for(slice_dir, &metadata.capability, Some(project_dir))
}

pub(super) fn resolve_tasks_path_for(
    slice_dir: &Path, capability_value: &str, project_hint: Option<&Path>,
) -> Result<PathBuf> {
    let project_dir = match project_hint {
        Some(p) => p.to_path_buf(),
        None => slice_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| Error::Diag {
                code: "slice-tasks-path-no-project-root",
                detail: format!(
                    "cannot resolve project root from slice dir {}",
                    slice_dir.display(),
                ),
            })?,
    };
    let pipeline = PipelineView::load(capability_value, &project_dir)?;
    let build_brief = pipeline.brief("build").ok_or_else(|| Error::Diag {
        code: "slice-tasks-build-brief-missing",
        detail: "capability has no `build` brief".to_string(),
    })?;
    let tracks_id = build_brief.frontmatter.tracks.as_deref().ok_or_else(|| Error::Diag {
        code: "slice-tasks-build-tracks-missing",
        detail: "`build` brief has no `tracks` field".to_string(),
    })?;
    let tracked = pipeline.brief(tracks_id).ok_or_else(|| Error::Diag {
        code: "slice-tasks-tracked-brief-missing",
        detail: format!("`build.tracks = {tracks_id}` but no such brief exists"),
    })?;
    let generates = brief_generates(tracked)?;
    Ok(slice_dir.join(generates))
}

fn brief_generates(brief: &Brief) -> Result<&str> {
    brief.frontmatter.generates.as_deref().ok_or_else(|| Error::Diag {
        code: "slice-tasks-brief-generates-missing",
        detail: format!("brief `{}` has no `generates` field", brief.frontmatter.id),
    })
}
