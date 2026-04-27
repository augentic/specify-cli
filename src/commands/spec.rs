use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use specify::{
    BaselineConflict, ContractAction, ContractPreviewEntry, MergeEntry, conflict_check, preview_change,
};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

use super::merge::{merge_op_to_json, operation_label, summarise_operations};

pub(crate) fn run_spec_preview(format: OutputFormat, change_dir: PathBuf) -> CliResult {
    let ctx = match CommandContext::require(format) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let result = match preview_change(&change_dir, &ctx.specs_dir()) {
        Ok(v) => v,
        Err(err) => return ctx.emit_error(&err),
    };

    match format {
        OutputFormat::Json => {
            let specs: Vec<Value> = result.specs.iter().map(preview_entry_to_json).collect();
            let contracts: Vec<Value> =
                result.contracts.iter().map(contract_preview_entry_to_json).collect();
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct SpecPreviewResponse {
                change_dir: String,
                specs: Vec<Value>,
                contracts: Vec<Value>,
            }
            emit_response(SpecPreviewResponse {
                change_dir: change_dir.display().to_string(),
                specs,
                contracts,
            });
        }
        OutputFormat::Text => {
            if result.specs.is_empty() {
                println!("No delta specs to merge.");
            } else {
                for entry in &result.specs {
                    println!(
                        "{}: {}",
                        entry.spec_name,
                        summarise_operations(&entry.result.operations)
                    );
                    for op in &entry.result.operations {
                        println!("  {}", operation_label(op));
                    }
                }
            }
            if !result.contracts.is_empty() {
                println!("\nContract changes:");
                for c in &result.contracts {
                    let (sigil, label) = match c.action {
                        ContractAction::Added => ("+", "added"),
                        ContractAction::Replaced => ("~", "replaced"),
                            _ => unreachable!(),
                    };
                    println!("  {sigil} contracts/{} ({label})", c.relative_path);
                }
            }
        }
    }
    CliResult::Success
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PreviewEntryJson {
    name: String,
    baseline_path: String,
    operations: Vec<Value>,
}

pub(crate) fn preview_entry_to_json(entry: &MergeEntry) -> Value {
    let ops: Vec<Value> = entry.result.operations.iter().map(merge_op_to_json).collect();
    serde_json::to_value(PreviewEntryJson {
        name: entry.spec_name.clone(),
        baseline_path: entry.baseline_path.display().to_string(),
        operations: ops,
    }).expect("PreviewEntryJson serialises")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContractPreviewJson {
    path: String,
    action: &'static str,
}

pub(crate) fn contract_preview_entry_to_json(entry: &ContractPreviewEntry) -> Value {
    let action = match entry.action {
        ContractAction::Added => "added",
        ContractAction::Replaced => "replaced",
            _ => unreachable!(),
    };
    serde_json::to_value(ContractPreviewJson {
        path: entry.relative_path.clone(),
        action,
    }).expect("ContractPreviewJson serialises")
}

pub(crate) fn run_spec_conflict_check(format: OutputFormat, change_dir: PathBuf) -> CliResult {
    let ctx = match CommandContext::require(format) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let conflicts = match conflict_check(&change_dir, &ctx.specs_dir()) {
        Ok(v) => v,
        Err(err) => return ctx.emit_error(&err),
    };

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ConflictCheckResponse {
                change_dir: String,
                conflicts: Vec<Value>,
            }
            let items: Vec<Value> = conflicts.iter().map(baseline_conflict_to_json).collect();
            emit_response(ConflictCheckResponse {
                change_dir: change_dir.display().to_string(),
                conflicts: items,
            });
        }
        OutputFormat::Text => {
            if conflicts.is_empty() {
                println!("No baseline conflicts.");
            } else {
                for c in &conflicts {
                    println!(
                        "{}: baseline modified {} (defined_at {})",
                        c.capability,
                        c.baseline_modified_at.format("%Y-%m-%dT%H:%M:%SZ"),
                        c.defined_at,
                    );
                }
            }
        }
    }
    CliResult::Success
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct BaselineConflictJson {
    capability: String,
    defined_at: String,
    baseline_modified_at: String,
}

pub(crate) fn baseline_conflict_to_json(c: &BaselineConflict) -> Value {
    serde_json::to_value(BaselineConflictJson {
        capability: c.capability.clone(),
        defined_at: c.defined_at.clone(),
        baseline_modified_at: c.baseline_modified_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    }).expect("BaselineConflictJson serialises")
}

