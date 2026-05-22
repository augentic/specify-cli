//! Single-slice status (`slice status`). Exposes the helpers consumed
//! by the top-level `specify status` dashboard so multi-slice
//! enumeration goes through one canonical reader.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_domain::slice::{LifecycleStatus, SliceMetadata};
use specify_domain::task::parse_tasks;
use specify_error::Result;

use crate::context::Ctx;

/// RFC-25 canonical refine-time artifacts probed for slice completion.
/// Mirrors [`specify_domain::validate::validate_slice`]'s artifact set.
const CANONICAL_ARTIFACTS: &[&str] = &["proposal.md", "spec.md", "design.md", "tasks.md"];

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::commands) struct StatusEntry {
    pub name: String,
    pub status: LifecycleStatus,
    pub target: String,
    pub tasks: Option<TaskCounts>,
    pub artifacts: BTreeMap<String, bool>,
}

#[derive(Serialize, Copy, Clone)]
#[serde(rename_all = "kebab-case")]
pub(in crate::commands) struct TaskCounts {
    pub total: usize,
    pub complete: usize,
}

pub(in crate::commands) fn collect_status(slice_dir: &Path, name: &str) -> Result<StatusEntry> {
    let metadata = SliceMetadata::load(slice_dir)?;

    let artifacts = canonical_artifact_completion(slice_dir);

    let tasks_path = slice_dir.join("tasks.md");
    let tasks = if tasks_path.is_file() {
        let content = std::fs::read_to_string(&tasks_path)?;
        let progress = parse_tasks(&content);
        Some(TaskCounts {
            total: progress.total,
            complete: progress.complete,
        })
    } else {
        None
    };

    Ok(StatusEntry {
        name: name.to_string(),
        status: metadata.status,
        target: metadata.target,
        tasks,
        artifacts,
    })
}

fn canonical_artifact_completion(slice_dir: &Path) -> BTreeMap<String, bool> {
    CANONICAL_ARTIFACTS
        .iter()
        .map(|artifact| ((*artifact).to_string(), slice_dir.join(artifact).is_file()))
        .collect()
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

pub(super) fn status_one(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    let entry = collect_status(&slice_dir, name)?;

    ctx.write(&StatusBody { slice: &entry }, write_status_text)?;
    Ok(())
}

#[derive(Serialize)]
struct StatusBody<'a> {
    slice: &'a StatusEntry,
}

fn write_status_text(w: &mut dyn Write, body: &StatusBody<'_>) -> std::io::Result<()> {
    let e = body.slice;
    writeln!(w, "{}", e.name)?;
    writeln!(w, "  target: {}", e.target)?;
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
