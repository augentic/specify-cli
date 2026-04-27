#![allow(clippy::needless_pass_by_value)]

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{ChangeMetadata, Error, Phase, PipelineView, parse_tasks};

use super::task::resolve_tasks_path_for;
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_status(ctx: &CommandContext, change: Option<String>) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let changes_dir = ctx.changes_dir();

    let names: Vec<String> = match &change {
        Some(n) => vec![n.clone()],
        None => list_change_names(&changes_dir)?,
    };

    let mut entries: Vec<StatusEntry> = Vec::new();
    for name in names {
        let dir = changes_dir.join(&name);
        let entry = collect_status(&dir, &name, &pipeline, &ctx.project_dir)?;
        entries.push(entry);
    }

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct StatusResponse {
                changes: Vec<Value>,
            }
            let changes: Vec<Value> = entries.iter().map(status_entry_to_json).collect();
            emit_response(StatusResponse { changes });
        }
        OutputFormat::Text => print_status_text(&entries),
    }
    Ok(CliResult::Success)
}

struct StatusEntry {
    name: String,
    schema: String,
    status: String,
    tasks: Option<(usize, usize)>,
    artifacts: std::collections::BTreeMap<String, bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct StatusEntryJson {
    name: String,
    status: String,
    schema: String,
    tasks: Option<TasksJson>,
    artifacts: std::collections::BTreeMap<String, bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TasksJson {
    total: usize,
    complete: usize,
}

fn status_entry_to_json(e: &StatusEntry) -> Value {
    let tasks_value = e.tasks.map(|(complete, total)| TasksJson { total, complete });
    serde_json::to_value(StatusEntryJson {
        name: e.name.clone(),
        status: e.status.clone(),
        schema: e.schema.clone(),
        tasks: tasks_value,
        artifacts: e.artifacts.clone(),
    })
    .expect("StatusEntryJson serialises")
}

fn collect_status(
    change_dir: &Path, name: &str, pipeline: &PipelineView, project_dir: &Path,
) -> Result<StatusEntry, Error> {
    let metadata = ChangeMetadata::load(change_dir)?;
    let status_str = metadata.status.to_string();

    // Delegate per-brief artifact completion to `PipelineView` so every
    // consumer — `specify status`, `specify schema pipeline`, and any
    // future skill callers — agrees on what "complete" means.
    let artifacts = pipeline.completion_for(Phase::Define, change_dir);

    let tasks = match resolve_tasks_path_for(change_dir, &metadata.schema, Some(project_dir)) {
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
        schema: metadata.schema,
        status: status_str,
        tasks,
        artifacts,
    })
}

fn list_change_names(changes_dir: &Path) -> Result<Vec<String>, Error> {
    if !changes_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(changes_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if !ChangeMetadata::path(&path).exists() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn print_status_text(entries: &[StatusEntry]) {
    if entries.is_empty() {
        println!("No changes.");
        return;
    }
    // Single-change detailed output.
    if entries.len() == 1 {
        let e = &entries[0];
        println!("{}", e.name);
        println!("  schema: {}", e.schema);
        println!("  status: {}", e.status);
        match e.tasks {
            Some((complete, total)) => println!("  tasks: {complete}/{total}"),
            None => println!("  tasks: (no tasks.md)"),
        }
        if !e.artifacts.is_empty() {
            println!("  artifacts:");
            for (k, present) in &e.artifacts {
                let mark = if *present { "x" } else { " " };
                println!("    [{mark}] {k}");
            }
        }
        return;
    }

    // Multi-change table.
    let name_w = entries.iter().map(|e| e.name.len()).max().unwrap_or(6).max(6);
    let status_w = entries.iter().map(|e| e.status.len()).max().unwrap_or(6).max(6);
    println!(
        "{:<name_w$}  {:<status_w$}  tasks",
        "change",
        "status",
        name_w = name_w,
        status_w = status_w
    );
    for e in entries {
        let tasks = match e.tasks {
            Some((complete, total)) => format!("{complete}/{total}"),
            None => "-".to_string(),
        };
        println!(
            "{:<name_w$}  {:<status_w$}  {}",
            e.name,
            e.status,
            tasks,
            name_w = name_w,
            status_w = status_w
        );
    }
}
