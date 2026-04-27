#![allow(clippy::items_after_statements, clippy::needless_pass_by_value)]

use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use specify::{
    ChangeMetadata, CreateIfExists, CreateOutcome, EntryKind, Error, Journal, JournalEntry,
    LifecycleStatus, Outcome, Phase, ProjectConfig, Rfc3339Stamp, SpecType, TouchedSpec,
    change_actions, format_rfc3339,
};

use super::status::run_status;
use crate::cli::{ChangeAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_change(ctx: &CommandContext, action: ChangeAction) -> Result<CliResult, Error> {
    match action {
        ChangeAction::Create {
            name,
            schema,
            if_exists,
        } => run_change_create(ctx, name, schema, if_exists.into()),
        ChangeAction::List => run_status(ctx, None),
        ChangeAction::Status { name } => run_status(ctx, Some(name)),
        ChangeAction::Transition { name, target } => run_change_transition(ctx, name, target),
        ChangeAction::TouchedSpecs { name, scan, set } => {
            run_change_touched_specs(ctx, name, scan, set)
        }
        ChangeAction::Overlap { name } => run_change_overlap(ctx, name),
        ChangeAction::Archive { name } => run_change_archive(ctx, name),
        ChangeAction::Drop { name, reason } => run_change_drop(ctx, name, reason),
        ChangeAction::PhaseOutcome {
            name,
            phase,
            outcome,
            summary,
            context,
        } => run_change_phase_outcome(ctx, name, phase, outcome, summary, context),
        ChangeAction::Outcome { name } => run_change_outcome(ctx, name),
        ChangeAction::JournalAppend {
            name,
            phase,
            kind,
            summary,
            context,
        } => run_change_journal_append(ctx, name, phase, kind, summary, context),
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
struct ChangeCreateResponse {
    name: String,
    change_dir: String,
    status: String,
    schema: String,
    created: bool,
    restarted: bool,
}

fn emit_change_create(format: OutputFormat, outcome: &CreateOutcome) -> CliResult {
    match format {
        OutputFormat::Json => emit_response(ChangeCreateResponse {
            name: outcome.change_dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            change_dir: outcome.change_dir.display().to_string(),
            status: outcome.metadata.status.to_string(),
            schema: outcome.metadata.schema.clone(),
            created: outcome.created,
            restarted: outcome.restarted,
        }),
        OutputFormat::Text => {
            if outcome.created {
                println!("Created change {}", outcome.change_dir.display());
            } else {
                println!("Reusing existing change {}", outcome.change_dir.display());
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
                    r#type: t.spec_type.to_string(),
                })
                .collect(),
        }),
        OutputFormat::Text => {
            if entries.is_empty() {
                println!("{name}: no touched specs");
            } else {
                println!("{name}:");
                for entry in &entries {
                    println!("  {} ({})", entry.name, entry.spec_type);
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
        let spec_type = match kind {
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
            spec_type,
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
                    other_change: o.other_change.clone(),
                    our_spec_type: o.our_spec_type.to_string(),
                    other_spec_type: o.other_spec_type.to_string(),
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
                        o.capability, o.other_change, o.our_spec_type, o.other_spec_type,
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

fn run_change_phase_outcome(
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
    struct PhaseOutcomeResponse {
        change: String,
        phase: String,
        outcome: String,
        at: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PhaseOutcomeResponse {
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
/// Symmetric with [`run_change_phase_outcome`] (the writer): this is
/// the read verb `/spec:execute` consumes after a phase returns.
/// Emits `"outcome": null` when the change exists but nothing has
/// been stamped; exits `CliResult::Success` in both cases — an unstamped
/// change is not an error, just an absence.
///
/// Falls back to `.specify/archive/` when the change is not found under
/// `.specify/changes/`. This handles the post-merge case: `specify merge`
/// stamps the outcome into `.metadata.yaml` and then archives the change
/// directory, so the active path no longer exists. The fallback scans
/// archive entries matching `*-<name>` and picks the most recent by
/// `created-at` timestamp.
fn run_change_outcome(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
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
                outcome: Option<OutcomeDetail>,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct OutcomeDetail {
                phase: String,
                outcome: String,
                at: Rfc3339Stamp,
                summary: String,
                context: Value,
            }
            let outcome_detail = metadata.outcome.as_ref().map(|o| OutcomeDetail {
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
/// `created-at`. Used by `run_change_outcome` as a fallback when the
/// active change directory has been archived by `specify merge`.
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

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(candidates.into_iter().next().unwrap().1)
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
    struct JournalAppendResponse {
        change: String,
        phase: String,
        kind: String,
        timestamp: Rfc3339Stamp,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(JournalAppendResponse {
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
