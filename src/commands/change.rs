#![allow(clippy::items_after_statements, clippy::needless_pass_by_value)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use specify::{
    BaselineConflict, Brief, ChangeMetadata, ContractAction, ContractPreviewEntry, CreateIfExists,
    CreateOutcome, EntryKind, Error, Journal, JournalEntry, LifecycleStatus, MergeEntry,
    MergeOperation, MergeResult, Outcome, Phase, PipelineView, ProjectConfig, Rfc3339Stamp,
    SpecType, Task, TouchedSpec, change_actions, conflict_check, format_rfc3339, mark_complete,
    merge_change, parse_tasks, preview_change, serialize_report, validate_change,
};

use crate::cli::{
    ChangeAction, ChangeMergeAction, ChangeTaskAction, JournalAction, OutcomeAction, OutputFormat,
};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_change(ctx: &CommandContext, action: ChangeAction) -> Result<CliResult, Error> {
    match action {
        ChangeAction::Create {
            name,
            schema,
            if_exists,
        } => run_change_create(ctx, name, schema, if_exists.into()),
        ChangeAction::List => run_change_list(ctx),
        ChangeAction::Status { name } => run_change_status_one(ctx, name),
        ChangeAction::Validate { name } => run_change_validate(ctx, name),
        ChangeAction::Merge { action } => match action {
            ChangeMergeAction::Run { name } => run_change_merge_run(ctx, name),
            ChangeMergeAction::Preview { name } => run_change_merge_preview(ctx, name),
            ChangeMergeAction::ConflictCheck { name } => run_change_merge_conflict_check(ctx, name),
        },
        ChangeAction::Task { action } => match action {
            ChangeTaskAction::Progress { name } => run_change_task_progress(ctx, name),
            ChangeTaskAction::Mark { name, task_number } => {
                run_change_task_mark(ctx, name, task_number)
            }
        },
        ChangeAction::Outcome { action } => match action {
            OutcomeAction::Set {
                name,
                phase,
                outcome,
                summary,
                context,
            } => run_change_outcome_set(ctx, name, phase, outcome, summary, context),
            OutcomeAction::Show { name } => run_change_outcome_show(ctx, name),
        },
        ChangeAction::Journal { action } => match action {
            JournalAction::Append {
                name,
                phase,
                kind,
                summary,
                context,
            } => run_change_journal_append(ctx, name, phase, kind, summary, context),
            JournalAction::Show { name } => run_change_journal_show(ctx, name),
        },
        ChangeAction::Transition { name, target } => run_change_transition(ctx, name, target),
        ChangeAction::TouchedSpecs { name, scan, set } => {
            run_change_touched_specs(ctx, name, scan, set)
        }
        ChangeAction::Overlap { name } => run_change_overlap(ctx, name),
        ChangeAction::Archive { name } => run_change_archive(ctx, name),
        ChangeAction::Drop { name, reason } => run_change_drop(ctx, name, reason),
    }
}

fn run_change_create(
    ctx: &CommandContext, name: String, schema: Option<String>, if_exists: CreateIfExists,
) -> Result<CliResult, Error> {
    let schema_value = schema.unwrap_or_else(|| ctx.config.schema.clone());
    let changes_dir = ctx.changes_dir();
    std::fs::create_dir_all(&changes_dir)?;

    let outcome =
        change_actions::create(&changes_dir, &name, &schema_value, if_exists, Utc::now())?;

    Ok(emit_change_create(ctx.format, &outcome))
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    name: String,
    change_dir: String,
    status: String,
    schema: String,
    created: bool,
    restarted: bool,
}

fn emit_change_create(format: OutputFormat, outcome: &CreateOutcome) -> CliResult {
    match format {
        OutputFormat::Json => emit_response(CreateBody {
            name: outcome.dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            change_dir: outcome.dir.display().to_string(),
            status: outcome.metadata.status.to_string(),
            schema: outcome.metadata.schema.clone(),
            created: outcome.created,
            restarted: outcome.restarted,
        }),
        OutputFormat::Text => {
            if outcome.created {
                println!("Created change {}", outcome.dir.display());
            } else {
                println!("Reusing existing change {}", outcome.dir.display());
            }
            if outcome.restarted {
                println!("  (previous directory was removed)");
            }
            println!("  schema: {}", outcome.metadata.schema);
            println!("  status: {}", outcome.metadata.status);
        }
    }
    CliResult::Success
}

fn run_change_transition(
    ctx: &CommandContext, name: String, target: LifecycleStatus,
) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let metadata = change_actions::transition(&change_dir, target, Utc::now())?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct TransitionResponse {
        name: String,
        status: String,
        defined_at: Option<Rfc3339Stamp>,
        build_started_at: Option<Rfc3339Stamp>,
        completed_at: Option<Rfc3339Stamp>,
        merged_at: Option<Rfc3339Stamp>,
        dropped_at: Option<Rfc3339Stamp>,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(TransitionResponse {
            name,
            status: metadata.status.to_string(),
            defined_at: metadata.defined_at.clone(),
            build_started_at: metadata.build_started_at.clone(),
            completed_at: metadata.completed_at.clone(),
            merged_at: metadata.merged_at.clone(),
            dropped_at: metadata.dropped_at,
        }),
        OutputFormat::Text => {
            println!("{name}: status = {}", metadata.status);
        }
    }
    Ok(CliResult::Success)
}

fn run_change_touched_specs(
    ctx: &CommandContext, name: String, scan: bool, set: Vec<String>,
) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let specs_dir = ctx.specs_dir();

    let entries = if !set.is_empty() {
        let v = parse_touched_spec_set(&set)?;
        let metadata = change_actions::write_touched_specs(&change_dir, v)?;
        metadata.touched_specs
    } else if scan {
        let scanned = change_actions::scan_touched_specs(&change_dir, &specs_dir)?;
        let metadata = change_actions::write_touched_specs(&change_dir, scanned)?;
        metadata.touched_specs
    } else {
        let metadata = ChangeMetadata::load(&change_dir)?;
        metadata.touched_specs
    };

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct TouchedSpecsResponse {
        name: String,
        touched_specs: Vec<TouchedSpecJson>,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(TouchedSpecsResponse {
            name,
            touched_specs: entries
                .iter()
                .map(|t| TouchedSpecJson {
                    name: t.name.clone(),
                    r#type: t.kind.to_string(),
                })
                .collect(),
        }),
        OutputFormat::Text => {
            if entries.is_empty() {
                println!("{name}: no touched specs");
            } else {
                println!("{name}:");
                for entry in &entries {
                    println!("  {} ({})", entry.name, entry.kind);
                }
            }
        }
    }
    Ok(CliResult::Success)
}

fn parse_touched_spec_set(raw: &[String]) -> Result<Vec<TouchedSpec>, Error> {
    let mut out: Vec<TouchedSpec> = Vec::with_capacity(raw.len());
    for entry in raw {
        let (name, kind) = entry.split_once(':').ok_or_else(|| {
            Error::Config(format!(
                "touched-specs entry `{entry}` must be `<name>:new` or `<name>:modified`"
            ))
        })?;
        let kind = match kind {
            "new" => SpecType::New,
            "modified" => SpecType::Modified,
            other => {
                return Err(Error::Config(format!(
                    "touched-specs kind `{other}` must be `new` or `modified`"
                )));
            }
        };
        out.push(TouchedSpec {
            name: name.to_string(),
            kind,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn run_change_overlap(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let changes_dir = ctx.changes_dir();
    let overlaps = change_actions::overlap(&changes_dir, &name)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct OverlapResponse {
        name: String,
        overlaps: Vec<OverlapJson>,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(OverlapResponse {
            name,
            overlaps: overlaps
                .iter()
                .map(|o| OverlapJson {
                    capability: o.capability.clone(),
                    other_change: o.other.clone(),
                    our_spec_type: o.ours.to_string(),
                    other_spec_type: o.theirs.to_string(),
                })
                .collect(),
        }),
        OutputFormat::Text => {
            if overlaps.is_empty() {
                println!("{name}: no overlapping changes");
            } else {
                for o in &overlaps {
                    println!(
                        "{}: also touched by `{}` ({} vs {})",
                        o.capability, o.other, o.ours, o.theirs,
                    );
                }
            }
        }
    }
    Ok(CliResult::Success)
}

fn run_change_archive(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let target = change_actions::archive(&change_dir, &archive_dir, Utc::now())?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct ArchiveResponse {
        name: String,
        archive_path: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(ArchiveResponse {
            name,
            archive_path: target.display().to_string(),
        }),
        OutputFormat::Text => {
            println!("{name}: archived to {}", target.display());
        }
    }
    Ok(CliResult::Success)
}

fn run_change_drop(
    ctx: &CommandContext, name: String, reason: Option<String>,
) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let (metadata, archive_path) =
        change_actions::drop(&change_dir, &archive_dir, reason.as_deref(), Utc::now())?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct DropResponse {
        name: String,
        status: String,
        archive_path: String,
        drop_reason: Option<String>,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(DropResponse {
            name,
            status: metadata.status.to_string(),
            archive_path: archive_path.display().to_string(),
            drop_reason: metadata.drop_reason,
        }),
        OutputFormat::Text => {
            println!("{name}: dropped and archived to {}", archive_path.display());
            if let Some(r) = &metadata.drop_reason {
                println!("  reason: {r}");
            }
        }
    }
    Ok(CliResult::Success)
}

fn run_change_outcome_set(
    ctx: &CommandContext, name: String, phase: Phase, outcome: Outcome, summary: String,
    context: Option<String>,
) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    if !change_dir.is_dir() || !ChangeMetadata::path(&change_dir).exists() {
        return Err(Error::ChangeNotFound { name });
    }

    let metadata = change_actions::phase_outcome(
        &change_dir,
        phase,
        outcome,
        &summary,
        context.as_deref(),
        Utc::now(),
    )?;

    let stamped = metadata
        .outcome
        .as_ref()
        .expect("phase_outcome action must set metadata.outcome on success");
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PhaseStamp {
        change: String,
        phase: String,
        outcome: String,
        at: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PhaseStamp {
            change: name,
            phase: phase.to_string(),
            outcome: outcome.to_string(),
            at: stamped.at.to_string(),
        }),
        OutputFormat::Text => {
            println!("Stamped outcome '{outcome}' for phase '{phase}' on change '{name}'.");
        }
    }
    Ok(CliResult::Success)
}

/// Report the stamped `.metadata.yaml.outcome` for `name`.
///
/// Symmetric with [`run_change_outcome_set`] (the writer): this is
/// the read verb `/spec:execute` consumes after a phase returns.
/// Emits `"outcome": null` when the change exists but nothing has
/// been stamped; exits `CliResult::Success` in both cases — an unstamped
/// change is not an error, just an absence.
///
/// Falls back to `.specify/archive/` when the change is not found under
/// `.specify/changes/`. This handles the post-merge case: `change merge run`
/// stamps the outcome into `.metadata.yaml` and then archives the change
/// directory, so the active path no longer exists. The fallback scans
/// archive entries matching `*-<name>` and picks the most recent by
/// `created-at` timestamp.
fn run_change_outcome_show(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let metadata = if change_dir.is_dir() {
        ChangeMetadata::load(&change_dir)?
    } else {
        resolve_archived_metadata(&ctx.project_dir, &name)?
    };

    match ctx.format {
        OutputFormat::Json => {
            // Build the outcome payload explicitly so `context` is
            // emitted as `null` when absent (the canonical shape
            // `/spec:execute` pattern-matches on). `PhaseOutcome`'s
            // serde derive skips `None` contexts on disk; the CLI
            // contract is the stable null.
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct OutcomeResponse {
                name: String,
                outcome: Option<OutcomeRow>,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct OutcomeRow {
                phase: String,
                outcome: String,
                at: Rfc3339Stamp,
                summary: String,
                context: Value,
            }
            let outcome_detail = metadata.outcome.as_ref().map(|o| OutcomeRow {
                phase: o.phase.to_string(),
                outcome: o.outcome.to_string(),
                at: o.at.clone(),
                summary: o.summary.clone(),
                context: o.context.clone().map_or(Value::Null, Value::from),
            });
            emit_response(OutcomeResponse {
                name,
                outcome: outcome_detail,
            });
        }
        OutputFormat::Text => match &metadata.outcome {
            Some(o) => {
                println!("{name}: {}/{} — {}", o.phase, o.outcome, o.summary);
            }
            None => {
                println!("{name}: no outcome stamped");
            }
        },
    }
    Ok(CliResult::Success)
}

/// Scan `.specify/archive/` for directories whose name ends with
/// `-<change_name>` (the `YYYY-MM-DD-<name>` convention), load each
/// candidate's `.metadata.yaml`, and return the most recent by
/// `created-at`. Used by `run_change_outcome_show` as a fallback when the
/// active change directory has been archived by `change merge run`.
fn resolve_archived_metadata(
    project_dir: &Path, change_name: &str,
) -> Result<ChangeMetadata, Error> {
    let archive_dir = ProjectConfig::archive_dir(project_dir);
    let suffix = format!("-{change_name}");
    let mut candidates: Vec<(String, ChangeMetadata)> = Vec::new();

    if archive_dir.is_dir() {
        let entries = std::fs::read_dir(&archive_dir)?;
        for entry in entries {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(&suffix) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if let Ok(meta) = ChangeMetadata::load(&entry.path()) {
                let created = meta.created_at.as_deref().unwrap_or("").to_string();
                candidates.push((created.clone(), meta));
            }
        }
    }

    if candidates.is_empty() {
        return Err(Error::ChangeNotFound {
            name: change_name.to_string(),
        });
    }

    let (_, metadata) = candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .expect("candidates is non-empty (checked above)");
    Ok(metadata)
}

fn run_change_journal_append(
    ctx: &CommandContext, name: String, phase: Phase, kind: EntryKind, summary: String,
    context: Option<String>,
) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    if !change_dir.is_dir() || !ChangeMetadata::path(&change_dir).exists() {
        return Err(Error::ChangeNotFound { name });
    }

    let timestamp = format_rfc3339(Utc::now());
    let entry = JournalEntry {
        timestamp: timestamp.clone(),
        step: phase,
        r#type: kind,
        summary,
        context,
    };

    Journal::append(&change_dir, entry)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct JournalBody {
        change: String,
        phase: String,
        kind: String,
        timestamp: Rfc3339Stamp,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(JournalBody {
            change: name,
            phase: phase.to_string(),
            kind: kind.to_string(),
            timestamp,
        }),
        OutputFormat::Text => {
            println!("Appended {kind} entry to {name}/journal.yaml.");
        }
    }
    Ok(CliResult::Success)
}

fn run_change_journal_show(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    if !change_dir.is_dir() || !ChangeMetadata::path(&change_dir).exists() {
        return Err(Error::ChangeNotFound { name });
    }

    let journal = Journal::load(&change_dir)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct JournalShowBody {
        name: String,
        entries: Vec<JournalEntryRow>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct JournalEntryRow {
        timestamp: Rfc3339Stamp,
        phase: String,
        kind: String,
        summary: String,
        context: Value,
    }

    match ctx.format {
        OutputFormat::Json => {
            let entries: Vec<JournalEntryRow> = journal
                .entries
                .iter()
                .map(|e| JournalEntryRow {
                    timestamp: e.timestamp.clone(),
                    phase: e.step.to_string(),
                    kind: e.r#type.to_string(),
                    summary: e.summary.clone(),
                    context: e.context.clone().map_or(Value::Null, Value::from),
                })
                .collect();
            emit_response(JournalShowBody { name, entries });
        }
        OutputFormat::Text => {
            if journal.entries.is_empty() {
                println!("{name}: no journal entries");
            } else {
                println!("{name}:");
                for entry in &journal.entries {
                    println!(
                        "  [{}] {}/{} — {}",
                        entry.timestamp, entry.step, entry.r#type, entry.summary,
                    );
                    if let Some(context) = &entry.context {
                        for line in context.lines() {
                            println!("      {line}");
                        }
                    }
                }
            }
        }
    }
    Ok(CliResult::Success)
}

// ---------------------------------------------------------------------------
// change validate
// ---------------------------------------------------------------------------

fn run_change_validate(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let pipeline = ctx.load_pipeline()?;
    let report = validate_change(&change_dir, &pipeline)?;

    match ctx.format {
        OutputFormat::Json => emit_response(serialize_report(&report)),
        OutputFormat::Text => print_validate_report(&report),
    }

    Ok(if report.passed { CliResult::Success } else { CliResult::ValidationFailed })
}

fn print_validate_report(report: &specify::ValidationReport) {
    println!("{}", if report.passed { "PASS" } else { "FAIL" });
    for (key, results) in &report.brief_results {
        println!("{key}:");
        for r in results {
            println!("  {}", format_result_line(r));
        }
    }
    if !report.cross_checks.is_empty() {
        println!("cross_checks:");
        for r in &report.cross_checks {
            println!("  {}", format_result_line(r));
        }
    }
}

fn format_result_line(r: &specify::ValidationResult) -> String {
    use specify::ValidationResult;
    match r {
        ValidationResult::Pass { rule_id, .. } => format!("[ok] {rule_id}"),
        ValidationResult::Fail { rule_id, detail, .. } => format!("[fail] {rule_id}: {detail}"),
        ValidationResult::Deferred { rule_id, reason, .. } => {
            format!("[defer] {rule_id} ({reason})")
        }
        _ => "[?] unknown validation result".to_string(),
    }
}

// ---------------------------------------------------------------------------
// change merge run / preview / conflict-check
// ---------------------------------------------------------------------------

/// RFC-3b: Detect whether a project directory is inside a workspace clone.
/// Two-part heuristic: (1) the path contains `/.specify/workspace/*/` as an
/// ancestor via structural component walk, and (2) `.specify/project.yaml`
/// exists in the project directory. The secondary guard — CWD does not
/// contain `.specify/plan.yaml` — is retained as a safety check but is not
/// sufficient on its own because `plan.yaml` may be absent after
/// `specify plan archive`.
fn is_workspace_clone(project_dir: &Path) -> bool {
    let components: Vec<_> = project_dir.components().collect();
    let in_workspace = components.windows(3).any(|w| {
        w[0].as_os_str() == ".specify"
            && w[1].as_os_str() == "workspace"
            && !w[2].as_os_str().is_empty()
    });
    if !in_workspace {
        return false;
    }
    let has_project_yaml = project_dir.join(".specify").join("project.yaml").exists();
    let has_plan_yaml = project_dir.join(".specify").join("plan.yaml").exists();
    has_project_yaml && !has_plan_yaml
}

fn run_change_merge_run(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let specs_dir = ctx.specs_dir();
    let archive_dir = ctx.archive_dir();

    let merged = merge_change(&change_dir, &specs_dir, &archive_dir)?;

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
                let commit_msg = format!("specify: merge {name}");
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
    let archive_path = archive_dir.join(format!("{today}-{name}"));

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct MergeResponse {
                merged_specs: Vec<Value>,
            }
            let specs: Vec<Value> = merged.iter().map(merge_entry_to_json).collect();
            emit_response(MergeResponse { merged_specs: specs });
        }
        OutputFormat::Text => {
            for (entry_name, result) in &merged {
                println!("{entry_name}: {}", summarise_operations(&result.operations));
            }
            println!("Archived to {}", archive_path.display());
        }
    }
    Ok(CliResult::Success)
}

fn run_change_merge_preview(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let result = preview_change(&change_dir, &ctx.specs_dir())?;

    match ctx.format {
        OutputFormat::Json => {
            let specs: Vec<Value> = result.specs.iter().map(preview_entry_to_json).collect();
            let contracts: Vec<Value> = result.contracts.iter().map(contract_to_json).collect();
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PreviewBody {
                change_dir: String,
                specs: Vec<Value>,
                contracts: Vec<Value>,
            }
            emit_response(PreviewBody {
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
                    println!("{}: {}", entry.name, summarise_operations(&entry.result.operations));
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
                        _ => ("?", "unknown"),
                    };
                    println!("  {sigil} contracts/{} ({label})", c.relative_path);
                }
            }
        }
    }
    Ok(CliResult::Success)
}

fn run_change_merge_conflict_check(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let conflicts = conflict_check(&change_dir, &ctx.specs_dir())?;

    match ctx.format {
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
    Ok(CliResult::Success)
}

// ---------------------------------------------------------------------------
// merge JSON helpers (lifted from the old `merge.rs` / `spec.rs`)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct MergeEntryJson {
    name: String,
    operations: Vec<Value>,
}

fn merge_entry_to_json(entry: &(String, MergeResult)) -> Value {
    let (name, result) = entry;
    let ops: Vec<Value> = result.operations.iter().map(merge_op_to_json).collect();
    serde_json::to_value(MergeEntryJson {
        name: name.clone(),
        operations: ops,
    })
    .expect("MergeEntryJson serialises")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SpecPreviewEntryJson {
    name: String,
    baseline_path: String,
    operations: Vec<Value>,
}

fn preview_entry_to_json(entry: &MergeEntry) -> Value {
    let ops: Vec<Value> = entry.result.operations.iter().map(merge_op_to_json).collect();
    serde_json::to_value(SpecPreviewEntryJson {
        name: entry.name.clone(),
        baseline_path: entry.baseline_path.display().to_string(),
        operations: ops,
    })
    .expect("SpecPreviewEntryJson serialises")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContractItem {
    path: String,
    action: &'static str,
}

fn contract_to_json(entry: &ContractPreviewEntry) -> Value {
    let action = match entry.action {
        ContractAction::Added => "added",
        ContractAction::Replaced => "replaced",
        _ => "unknown",
    };
    serde_json::to_value(ContractItem {
        path: entry.relative_path.clone(),
        action,
    })
    .expect("ContractItem serialises")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConflictRow {
    capability: String,
    defined_at: String,
    baseline_modified_at: String,
}

fn baseline_conflict_to_json(c: &BaselineConflict) -> Value {
    serde_json::to_value(ConflictRow {
        capability: c.capability.clone(),
        defined_at: c.defined_at.clone(),
        baseline_modified_at: c.baseline_modified_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    })
    .expect("ConflictRow serialises")
}

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
        // `MergeOperation` is #[non_exhaustive]; update when adding variants.
        _ => "UNKNOWN operation".to_string(),
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

fn merge_op_to_json(op: &MergeOperation) -> Value {
    let typed = match op {
        MergeOperation::Added { id, name } => MergeOpJson::Added {
            id: id.clone(),
            name: name.clone(),
        },
        MergeOperation::Modified { id, name } => MergeOpJson::Modified {
            id: id.clone(),
            name: name.clone(),
        },
        MergeOperation::Removed { id, name } => MergeOpJson::Removed {
            id: id.clone(),
            name: name.clone(),
        },
        MergeOperation::Renamed {
            id,
            old_name,
            new_name,
        } => MergeOpJson::Renamed {
            id: id.clone(),
            old_name: old_name.clone(),
            new_name: new_name.clone(),
        },
        MergeOperation::CreatedBaseline { requirement_count } => MergeOpJson::CreatedBaseline {
            requirement_count: *requirement_count,
        },
        // `MergeOperation` is #[non_exhaustive]; update when adding variants.
        _ => {
            return serde_json::to_value(serde_json::json!({"kind": "unknown"}))
                .expect("fallback JSON serialises");
        }
    };
    serde_json::to_value(typed).expect("MergeOpJson serialises")
}

fn summarise_operations(ops: &[MergeOperation]) -> String {
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
            _ => {}
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
// change task (progress / mark)
// ---------------------------------------------------------------------------

fn run_change_task_progress(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let tasks_path = resolve_tasks_path(&ctx.project_dir, &change_dir)?;
    let content = std::fs::read_to_string(&tasks_path)?;
    let progress = parse_tasks(&content);

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ProgressBody {
                total: usize,
                complete: usize,
                pending: usize,
                tasks: Vec<Value>,
            }
            let tasks: Vec<Value> = progress.tasks.iter().map(task_to_json).collect();
            emit_response(ProgressBody {
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
    Ok(CliResult::Success)
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DirectiveRow {
    plugin: String,
    skill: String,
}

fn task_to_json(t: &Task) -> Value {
    let skill = t.skill_directive.as_ref().map(|d| DirectiveRow {
        plugin: d.plugin.clone(),
        skill: d.skill.clone(),
    });
    serde_json::to_value(TaskRow {
        group: t.group.clone(),
        number: t.number.clone(),
        description: t.description.clone(),
        complete: t.complete,
        skill_directive: skill,
    })
    .expect("TaskRow serialises")
}

fn run_change_task_mark(
    ctx: &CommandContext, name: String, task_number: String,
) -> Result<CliResult, Error> {
    let change_dir = ctx.changes_dir().join(&name);
    let tasks_path = resolve_tasks_path(&ctx.project_dir, &change_dir)?;
    let original = std::fs::read_to_string(&tasks_path)?;
    let updated = mark_complete(&original, &task_number)?;
    let idempotent = updated == original;
    if !idempotent {
        std::fs::write(&tasks_path, &updated)?;
    }

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct MarkBody {
                marked: String,
                new_content_path: String,
                idempotent: bool,
            }
            emit_response(MarkBody {
                marked: task_number,
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
    Ok(CliResult::Success)
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

pub fn resolve_tasks_path_for(
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

// ---------------------------------------------------------------------------
// change list / change status (multi-change list + single-change view)
// ---------------------------------------------------------------------------

pub(super) struct StatusEntry {
    pub name: String,
    pub schema: String,
    pub status: String,
    pub tasks: Option<(usize, usize)>,
    pub artifacts: BTreeMap<String, bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryJson {
    name: String,
    status: String,
    schema: String,
    tasks: Option<TaskCounts>,
    artifacts: BTreeMap<String, bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TaskCounts {
    total: usize,
    complete: usize,
}

pub(super) fn status_entry_to_json(e: &StatusEntry) -> Value {
    let tasks_value = e.tasks.map(|(complete, total)| TaskCounts { total, complete });
    serde_json::to_value(EntryJson {
        name: e.name.clone(),
        status: e.status.clone(),
        schema: e.schema.clone(),
        tasks: tasks_value,
        artifacts: e.artifacts.clone(),
    })
    .expect("EntryJson serialises")
}

pub(super) fn collect_status(
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

pub(super) fn list_change_names(changes_dir: &Path) -> Result<Vec<String>, Error> {
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

fn run_change_list(ctx: &CommandContext) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let changes_dir = ctx.changes_dir();
    let names = list_change_names(&changes_dir)?;

    let mut entries: Vec<StatusEntry> = Vec::with_capacity(names.len());
    for name in names {
        let dir = changes_dir.join(&name);
        let entry = collect_status(&dir, &name, &pipeline, &ctx.project_dir)?;
        entries.push(entry);
    }

    emit_change_list(ctx.format, &entries);
    Ok(CliResult::Success)
}

fn run_change_status_one(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let change_dir = ctx.changes_dir().join(&name);
    let entry = collect_status(&change_dir, &name, &pipeline, &ctx.project_dir)?;

    emit_change_list(ctx.format, std::slice::from_ref(&entry));
    Ok(CliResult::Success)
}

fn emit_change_list(format: OutputFormat, entries: &[StatusEntry]) {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct StatusResponse {
                changes: Vec<Value>,
            }
            let changes: Vec<Value> = entries.iter().map(status_entry_to_json).collect();
            emit_response(StatusResponse { changes });
        }
        OutputFormat::Text => print_change_list_text(entries),
    }
}

fn print_change_list_text(entries: &[StatusEntry]) {
    if entries.is_empty() {
        println!("No changes.");
        return;
    }
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OverlapJson {
    capability: String,
    other_change: String,
    our_spec_type: String,
    other_spec_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TouchedSpecJson {
    name: String,
    r#type: String,
}

#[cfg(test)]
mod merge_workspace_tests {
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
