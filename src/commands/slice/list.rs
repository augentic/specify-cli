//! Multi-slice list (`slice list`) and single-slice status (`slice status`).
//!
//! Also exposes [`StatusEntry`], [`collect_status`], [`list_slice_names`],
//! and [`status_entry_to_json`] for the top-level `specify status` dashboard
//! in `super::super::status`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify_capability::{Phase, PipelineView};
use specify_error::Result;
use specify_slice::SliceMetadata;
use specify_task::parse_tasks;

use crate::context::CommandContext;
use crate::output::{Render, emit};

pub(in crate::commands) struct StatusEntry {
    pub name: String,
    pub capability: String,
    pub status: String,
    pub tasks: Option<(usize, usize)>,
    pub artifacts: BTreeMap<String, bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryJson {
    name: String,
    status: String,
    capability: String,
    tasks: Option<TaskCounts>,
    artifacts: BTreeMap<String, bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TaskCounts {
    total: usize,
    complete: usize,
}

pub(in crate::commands) fn status_entry_to_json(e: &StatusEntry) -> Value {
    let tasks_value = e.tasks.map(|(complete, total)| TaskCounts { total, complete });
    serde_json::to_value(EntryJson {
        name: e.name.clone(),
        status: e.status.clone(),
        capability: e.capability.clone(),
        tasks: tasks_value,
        artifacts: e.artifacts.clone(),
    })
    .expect("EntryJson serialises")
}

pub(in crate::commands) fn collect_status(
    slice_dir: &Path, name: &str, pipeline: &PipelineView, project_dir: &Path,
) -> Result<StatusEntry> {
    let metadata = SliceMetadata::load(slice_dir)?;
    let status_str = metadata.status.to_string();

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
                Some((progress.complete, progress.total))
            } else {
                None
            }
        }
        Err(_) => None,
    };

    Ok(StatusEntry {
        name: name.to_string(),
        capability: metadata.capability,
        status: status_str,
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

pub(super) fn run(ctx: &CommandContext) -> Result<()> {
    let pipeline = ctx.load_pipeline()?;
    let slices_dir = ctx.slices_dir();
    let names = list_slice_names(&slices_dir)?;

    let mut entries: Vec<StatusEntry> = Vec::with_capacity(names.len());
    for name in names {
        let dir = slices_dir.join(&name);
        let entry = collect_status(&dir, &name, &pipeline, &ctx.project_dir)?;
        entries.push(entry);
    }

    emit(ctx.format, &StatusListBody::new(&entries))?;
    Ok(())
}

pub(super) fn status_one(ctx: &CommandContext, name: String) -> Result<()> {
    let pipeline = ctx.load_pipeline()?;
    let slice_dir = ctx.slices_dir().join(&name);
    let entry = collect_status(&slice_dir, &name, &pipeline, &ctx.project_dir)?;

    emit(ctx.format, &StatusListBody::new(std::slice::from_ref(&entry)))?;
    Ok(())
}

struct StatusListBody<'a> {
    entries: &'a [StatusEntry],
}

impl<'a> StatusListBody<'a> {
    const fn new(entries: &'a [StatusEntry]) -> Self {
        Self { entries }
    }
}

impl Serialize for StatusListBody<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let slices: Vec<Value> = self.entries.iter().map(status_entry_to_json).collect();
        let mut s = serializer.serialize_struct("StatusListBody", 1)?;
        s.serialize_field("slices", &slices)?;
        s.end()
    }
}

impl Render for StatusListBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.entries.is_empty() {
            return writeln!(w, "No slices.");
        }
        if self.entries.len() == 1 {
            return render_single(w, &self.entries[0]);
        }
        render_table(w, self.entries)
    }
}

fn render_single(w: &mut dyn Write, e: &StatusEntry) -> std::io::Result<()> {
    writeln!(w, "{}", e.name)?;
    writeln!(w, "  capability: {}", e.capability)?;
    writeln!(w, "  status: {}", e.status)?;
    match e.tasks {
        Some((complete, total)) => writeln!(w, "  tasks: {complete}/{total}")?,
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
    let status_w = entries.iter().map(|e| e.status.len()).max().unwrap_or(6).max(6);
    writeln!(
        w,
        "{:<name_w$}  {:<status_w$}  tasks",
        "slice",
        "status",
        name_w = name_w,
        status_w = status_w,
    )?;
    for e in entries {
        let tasks = match e.tasks {
            Some((complete, total)) => format!("{complete}/{total}"),
            None => "-".to_string(),
        };
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
