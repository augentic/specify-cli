use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use specify::{Error, MergeOperation, MergeResult, merge_change};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

/// RFC-3b: Detect whether a project directory is inside a workspace clone.
/// Two-part heuristic: (1) the path contains `/.specify/workspace/*/` as an
/// ancestor, and (2) `.specify/project.yaml` exists in the project directory.
/// The secondary guard — CWD does not contain `.specify/plan.yaml` — is
/// retained as a safety check but is not sufficient on its own because
/// `plan.yaml` may be absent after `specify plan archive`.
fn is_workspace_clone(project_dir: &Path) -> bool {
    let in_workspace = project_dir
        .to_str()
        .map(|s| s.contains("/.specify/workspace/") || s.contains("\\.specify\\workspace\\"))
        .unwrap_or(false);
    if !in_workspace {
        return false;
    }
    let has_project_yaml = project_dir.join(".specify").join("project.yaml").exists();
    let has_plan_yaml = project_dir.join(".specify").join("plan.yaml").exists();
    has_project_yaml && !has_plan_yaml
}

pub(crate) fn run_merge(format: OutputFormat, change_dir: PathBuf) -> CliResult {
    let ctx = match CommandContext::require(format) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let specs_dir = ctx.specs_dir();
    let archive_dir = ctx.archive_dir();

    let change_name = match change_dir.file_name().and_then(|s| s.to_str()) {
        Some(name) => name.to_string(),
        None => {
            let err =
                Error::Config(format!("change dir `{}` has no basename", change_dir.display()));
            return ctx.emit_error(&err);
        }
    };

    let merged = match merge_change(&change_dir, &specs_dir, &archive_dir) {
        Ok(m) => m,
        Err(err) => return ctx.emit_error(&err),
    };

    // RFC-3b: auto-commit merged specs when running inside a workspace clone.
    if is_workspace_clone(&ctx.project_dir) {
        let specs_path = ctx.specs_dir();
        let archive_path_for_git = ctx.archive_dir();

        let git_add = std::process::Command::new("git")
            .arg("-C")
            .arg(&ctx.project_dir)
            .args(["add"])
            .arg(&specs_path)
            .arg(&archive_path_for_git)
            .output();

        match git_add {
            Ok(output) if output.status.success() => {
                let commit_msg = format!("specify: merge {change_name}");
                let git_commit = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&ctx.project_dir)
                    .args(["commit", "-m", &commit_msg])
                    .output();

                match git_commit {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        eprintln!(
                            "warning: workspace auto-commit failed (non-zero exit): {stderr}"
                        );
                    }
                    Err(err) => {
                        eprintln!("warning: workspace auto-commit failed: {err}");
                    }
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("warning: workspace git-add failed (non-zero exit): {stderr}");
            }
            Err(err) => {
                eprintln!("warning: workspace git-add failed: {err}");
            }
        }
    }

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let archive_path = archive_dir.join(format!("{today}-{change_name}"));

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct MergeResponse {
                merged_specs: Vec<Value>,
            }
            let specs: Vec<Value> = merged.iter().map(merge_entry_to_json).collect();
            emit_response(MergeResponse {
                merged_specs: specs,
            });
        }
        OutputFormat::Text => {
            for (name, result) in &merged {
                println!("{name}: {}", summarise_operations(&result.operations));
            }
            println!("Archived to {}", archive_path.display());
        }
    }
    CliResult::Success
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct MergeEntryJson {
    name: String,
    operations: Vec<Value>,
}

pub(crate) fn merge_entry_to_json(entry: &(String, MergeResult)) -> Value {
    let (name, result) = entry;
    let ops: Vec<Value> = result.operations.iter().map(merge_op_to_json).collect();
    serde_json::to_value(MergeEntryJson {
        name: name.clone(),
        operations: ops,
    }).expect("MergeEntryJson serialises")
}

pub(crate) fn operation_label(op: &MergeOperation) -> String {
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
            _ => unreachable!(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "kind")]
enum MergeOpJson {
    #[serde(rename = "added")]
    Added { id: String, name: String },
    #[serde(rename = "modified")]
    Modified { id: String, name: String },
    #[serde(rename = "removed")]
    Removed { id: String, name: String },
    #[serde(rename = "renamed")]
    Renamed { id: String, old_name: String, new_name: String },
    #[serde(rename = "created_baseline")]
    CreatedBaseline { requirement_count: usize },
}

pub(crate) fn merge_op_to_json(op: &MergeOperation) -> Value {
    let typed = match op {
        MergeOperation::Added { id, name } => MergeOpJson::Added {
            id: id.clone(), name: name.clone(),
        },
        MergeOperation::Modified { id, name } => MergeOpJson::Modified {
            id: id.clone(), name: name.clone(),
        },
        MergeOperation::Removed { id, name } => MergeOpJson::Removed {
            id: id.clone(), name: name.clone(),
        },
        MergeOperation::Renamed { id, old_name, new_name } => MergeOpJson::Renamed {
            id: id.clone(), old_name: old_name.clone(), new_name: new_name.clone(),
        },
        MergeOperation::CreatedBaseline { requirement_count } => MergeOpJson::CreatedBaseline {
            requirement_count: *requirement_count,
        },
            _ => unreachable!(),
    };
    serde_json::to_value(typed).expect("MergeOpJson serialises")
}

pub(crate) fn summarise_operations(ops: &[MergeOperation]) -> String {
    let mut added = 0;
    let mut modified = 0;
    let mut removed = 0;
    let mut renamed = 0;
    let mut created_baseline = None;
    for op in ops {
        match op {
            MergeOperation::Added { .. } => added += 1,
            MergeOperation::Modified { .. } => modified += 1,
            MergeOperation::Removed { .. } => removed += 1,
            MergeOperation::Renamed { .. } => renamed += 1,
            MergeOperation::CreatedBaseline { requirement_count } => {
                created_baseline = Some(*requirement_count);
            }
                _ => unreachable!(),
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

#[cfg(test)]
mod merge_workspace_tests {
    use super::*;
    use std::path::Path;

    fn workspace_clone_dir(suffix: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let slot = tmp.path().join(".specify").join("workspace").join(suffix);
        std::fs::create_dir_all(slot.join(".specify")).unwrap();
        std::fs::write(slot.join(".specify").join("project.yaml"), "name: stub\n").unwrap();
        tmp
    }

    #[test]
    fn detects_workspace_clone_unix_path() {
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
    fn rejects_initiating_repo_with_specify_dir() {
        let path = Path::new("/home/user/project/.specify/");
        assert!(!is_workspace_clone(path));
    }

    #[test]
    fn detects_deeply_nested_workspace_clone() {
        let tmp = workspace_clone_dir("mobile");
        let path =
            tmp.path().join(".specify").join("workspace").join("mobile").join("sub").join("dir");
        std::fs::create_dir_all(path.join(".specify")).unwrap();
        std::fs::write(path.join(".specify").join("project.yaml"), "name: stub\n").unwrap();
        assert!(is_workspace_clone(&path));
    }
}

