//! Multi-slice list (`slice list`) and single-slice status
//! (`slice status`). Exposes the helpers consumed by the top-level
//! `specify status` dashboard.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_domain::capability::{Phase, PipelineView};
use specify_domain::slice::{LifecycleStatus, SliceMetadata};
use specify_domain::task::parse_tasks;
use specify_error::Result;

use crate::context::Ctx;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::commands) struct StatusEntry {
    pub name: String,
    pub status: LifecycleStatus,
    pub capability: String,
    pub tasks: Option<TaskCounts>,
    pub artifacts: BTreeMap<String, bool>,
}

#[derive(Serialize, Copy, Clone)]
#[serde(rename_all = "kebab-case")]
pub(in crate::commands) struct TaskCounts {
    pub total: usize,
    pub complete: usize,
}

pub(in crate::commands) fn collect_status(
    slice_dir: &Path, name: &str, pipeline: &PipelineView, project_dir: &Path,
) -> Result<StatusEntry> {
    let metadata = SliceMetadata::load(slice_dir)?;

    // Delegate per-brief artifact completion to `PipelineView` so every
    // consumer agrees on what "complete" means.
    let artifacts = pipeline.completion_for(Phase::Define, slice_dir);

    let tasks = match super::task::resolve_tasks_path_for(
        slice_dir,
        &metadata.capability,
        Some(project_dir),
    ) {
        Ok(path) => {
            if path.is_file() {
                let content = std::fs::read_to_string(&path)?;
                let progress = parse_tasks(&content);
                Some(TaskCounts { total: progress.total, complete: progress.complete })
            } else {
                None
            }
        }
        Err(_) => None,
    };

    Ok(StatusEntry {
        name: name.to_string(),
        status: metadata.status,
        capability: metadata.capability,
        tasks,
        artifacts,
    })
}

pub(in crate::commands) fn list_slice_names(slices_dir: &Path) -> Result<Vec<String>> {
    if !slices_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(slices_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if !SliceMetadata::path(&path).exists() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

pub(super) fn run(ctx: &Ctx) -> Result<()> {
    let pipeline = ctx.load_pipeline()?;
    let slices_dir = ctx.slices_dir();
    let names = list_slice_names(&slices_dir)?;

    let mut entries: Vec<StatusEntry> = Vec::with_capacity(names.len());
    for name in names {
        let dir = slices_dir.join(&name);
        let entry = collect_status(&dir, &name, &pipeline, &ctx.project_dir)?;
        entries.push(entry);
    }

    ctx.write(&StatusBody::new(&entries), write_status_text)?;
    Ok(())
}

pub(super) fn status_one(ctx: &Ctx, name: &str) -> Result<()> {
    let pipeline = ctx.load_pipeline()?;
    let slice_dir = ctx.slices_dir().join(name);
    let entry = collect_status(&slice_dir, name, &pipeline, &ctx.project_dir)?;

    ctx.write(&StatusBody::new(std::slice::from_ref(&entry)), write_status_text)?;
    Ok(())
}

#[derive(Serialize)]
struct StatusBody<'a> {
    slices: &'a [StatusEntry],
}

impl<'a> StatusBody<'a> {
    const fn new(slices: &'a [StatusEntry]) -> Self {
        Self { slices }
    }
}

fn write_status_text(w: &mut dyn Write, body: &StatusBody<'_>) -> std::io::Result<()> {
    if body.slices.is_empty() {
        return writeln!(w, "No slices.");
    }
    if body.slices.len() == 1 {
        return render_single(w, &body.slices[0]);
    }
    render_table(w, body.slices)
}

fn render_single(w: &mut dyn Write, e: &StatusEntry) -> std::io::Result<()> {
    writeln!(w, "{}", e.name)?;
    writeln!(w, "  capability: {}", e.capability)?;
    writeln!(w, "  status: {}", e.status)?;
    match e.tasks {
        Some(tc) => writeln!(w, "  tasks: {}/{}", tc.complete, tc.total)?,
        None => writeln!(w, "  tasks: (no tasks.md)")?,
    }
    if !e.artifacts.is_empty() {
        writeln!(w, "  artifacts:")?;
        for (k, present) in &e.artifacts {
            let mark = if *present { "x" } else { " " };
            writeln!(w, "    [{mark}] {k}")?;
        }
    }
    Ok(())
}

fn render_table(w: &mut dyn Write, entries: &[StatusEntry]) -> std::io::Result<()> {
    let name_w = entries.iter().map(|e| e.name.len()).max().unwrap_or(6).max(6);
    let status_w = entries.iter().map(|e| e.status.to_string().len()).max().unwrap_or(6).max(6);
    writeln!(
        w,
        "{:<name_w$}  {:<status_w$}  tasks",
        "slice",
        "status",
        name_w = name_w,
        status_w = status_w,
    )?;
    for e in entries {
        let tasks = e.tasks.map_or_else(
            || "-".to_string(),
            |tc| format!("{}/{}", tc.complete, tc.total),
        );
        writeln!(
            w,
            "{:<name_w$}  {:<status_w$}  {}",
            e.name,
            e.status,
            tasks,
            name_w = name_w,
            status_w = status_w,
        )?;
    }
    Ok(())
}
