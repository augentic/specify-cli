use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify::{Brief, ChangeMetadata, Error, PipelineView, Task, mark_complete, parse_tasks};

use crate::cli::OutputFormat;
use crate::output::{CliResult, emit_error, emit_response};

use super::require_project;

pub(crate) fn run_task_progress(format: OutputFormat, change_dir: PathBuf) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let tasks_path = match resolve_tasks_path(&project_dir, &change_dir) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let content = match std::fs::read_to_string(&tasks_path) {
        Ok(t) => t,
        Err(err) => return emit_error(format, &Error::Io(err)),
    };
    let progress = parse_tasks(&content);

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct TaskProgressResponse {
                total: usize,
                complete: usize,
                pending: usize,
                tasks: Vec<Value>,
            }
            let tasks: Vec<Value> = progress.tasks.iter().map(task_to_json).collect();
            emit_response(TaskProgressResponse {
                total: progress.total,
                complete: progress.complete,
                pending: progress.total.saturating_sub(progress.complete),
                tasks,
            });
        }
        OutputFormat::Text => {
            println!("{}/{} tasks complete", progress.complete, progress.total);
            for task in &progress.tasks {
                let mark = if task.complete { "x" } else { " " };
                println!("  [{}] {} {}", mark, task.number, task.description);
            }
        }
    }
    CliResult::Success
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TaskJson {
    group: String,
    number: String,
    description: String,
    complete: bool,
    skill_directive: Option<SkillDirectiveJson>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SkillDirectiveJson {
    plugin: String,
    skill: String,
}

fn task_to_json(t: &Task) -> Value {
    let skill = t.skill_directive.as_ref().map(|d| SkillDirectiveJson {
        plugin: d.plugin.clone(),
        skill: d.skill.clone(),
    });
    serde_json::to_value(TaskJson {
        group: t.group.clone(),
        number: t.number.clone(),
        description: t.description.clone(),
        complete: t.complete,
        skill_directive: skill,
    }).expect("TaskJson serialises")
}

pub(crate) fn run_task_mark(format: OutputFormat, change_dir: PathBuf, task_number: String) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let tasks_path = match resolve_tasks_path(&project_dir, &change_dir) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let original = match std::fs::read_to_string(&tasks_path) {
        Ok(t) => t,
        Err(err) => return emit_error(format, &Error::Io(err)),
    };
    let updated = match mark_complete(&original, &task_number) {
        Ok(s) => s,
        Err(err) => return emit_error(format, &err),
    };
    let idempotent = updated == original;
    if !idempotent && let Err(err) = std::fs::write(&tasks_path, &updated) {
        return emit_error(format, &Error::Io(err));
    }

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct TaskMarkResponse {
                marked: String,
                new_content_path: String,
                idempotent: bool,
            }
            emit_response(TaskMarkResponse {
                marked: task_number.clone(),
                new_content_path: tasks_path.display().to_string(),
                idempotent,
            });
        }
        OutputFormat::Text => {
            if idempotent {
                println!("Task {task_number} already complete.");
            } else {
                println!("Marked task {task_number} complete.");
            }
        }
    }
    CliResult::Success
}

/// Resolve the `tasks.md` path for a change.
///
/// Walks the pipeline view to find the `build` brief's `tracks` value
/// (the id of the tasks brief), then uses that brief's `generates`
/// field as the relative path under `change_dir`. This lets the CLI
/// honour schemas that rename `tasks.md` or nest it elsewhere.
fn resolve_tasks_path(project_dir: &Path, change_dir: &Path) -> Result<PathBuf, Error> {
    let metadata = ChangeMetadata::load(change_dir)?;
    resolve_tasks_path_for(change_dir, &metadata.schema, Some(project_dir))
}

pub(crate) fn resolve_tasks_path_for(
    change_dir: &Path, schema_value: &str, project_hint: Option<&Path>,
) -> Result<PathBuf, Error> {
    // Use the hinted project dir when supplied; otherwise walk up from
    // the change dir — convention is `<project>/.specify/changes/<name>`.
    let project_dir = match project_hint {
        Some(p) => p.to_path_buf(),
        None => change_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                Error::Config(format!(
                    "cannot resolve project root from change dir {}",
                    change_dir.display()
                ))
            })?,
    };
    let pipeline = PipelineView::load(schema_value, &project_dir)?;
    let build_brief = pipeline
        .brief("build")
        .ok_or_else(|| Error::Config("schema has no `build` brief".to_string()))?;
    let tracks_id = build_brief
        .frontmatter
        .tracks
        .as_deref()
        .ok_or_else(|| Error::Config("`build` brief has no `tracks` field".to_string()))?;
    let tracked = pipeline.brief(tracks_id).ok_or_else(|| {
        Error::Config(format!("`build.tracks = {tracks_id}` but no such brief exists"))
    })?;
    let generates = brief_generates(tracked)?;
    Ok(change_dir.join(generates))
}

fn brief_generates(brief: &Brief) -> Result<&str, Error> {
    brief.frontmatter.generates.as_deref().ok_or_else(|| {
        Error::Config(format!("brief `{}` has no `generates` field", brief.frontmatter.id))
    })
}

