#![allow(
    clippy::items_after_statements,
    reason = "JSON DTOs sit close to their emission sites for readability."
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use specify::{
    ArtifactClass, BaselineConflict, Brief, CreateIfExists, CreateOutcome, EntryKind, Error,
    Journal, JournalEntry, LifecycleStatus, MergeOperation, MergePreviewEntry, MergeStrategy,
    OpaqueAction, OpaquePreviewEntry, Outcome, Phase, PipelineView, ProjectConfig, Rfc3339Stamp,
    SliceMetadata, SpecKind, Task, TouchedSpec, conflict_check, format_rfc3339,
    is_workspace_clone_path, mark_complete, merge_slice, parse_tasks, preview_slice,
    serialize_report, slice_actions, validate_slice,
};

use crate::cli::{
    JournalAction, OutcomeAction, OutcomeKindAction, OutputFormat, SliceAction, SliceMergeAction,
    SliceTaskAction,
};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

const WORKSPACE_MERGE_COMMIT_PATHS: [&str; 2] = [".specify/specs", ".specify/archive"];

/// Default omnia [`ArtifactClass`] set: `specs` (3-way merge) and
/// `contracts` (opaque replace). Single source of truth in the binary;
/// future capability manifests should drive this through `specify-capability`.
fn artifact_classes(project_root: &Path, slice_dir: &Path) -> Vec<ArtifactClass> {
    vec![
        ArtifactClass {
            name: "specs".to_string(),
            staged_dir: slice_dir.join("specs"),
            baseline_dir: ProjectConfig::specify_dir(project_root).join("specs"),
            strategy: MergeStrategy::ThreeWayMerge,
        },
        ArtifactClass {
            name: "contracts".to_string(),
            staged_dir: slice_dir.join("contracts"),
            baseline_dir: project_root.join("contracts"),
            strategy: MergeStrategy::OpaqueReplace,
        },
    ]
}

pub fn run(ctx: &CommandContext, action: SliceAction) -> Result<CliResult, Error> {
    match action {
        SliceAction::Create {
            name,
            capability,
            if_exists,
        } => create(ctx, name, capability, if_exists.into()),
        SliceAction::List => list(ctx),
        SliceAction::Status { name } => status_one(ctx, name),
        SliceAction::Validate { name } => validate(ctx, name),
        SliceAction::Merge { action } => match action {
            SliceMergeAction::Run { name } => merge_run(ctx, name),
            SliceMergeAction::Preview { name } => merge_preview(ctx, name),
            SliceMergeAction::ConflictCheck { name } => merge_conflict_check(ctx, name),
        },
        SliceAction::Task { action } => match action {
            SliceTaskAction::Progress { name } => task_progress(ctx, name),
            SliceTaskAction::Mark { name, task_number } => task_mark(ctx, name, task_number),
        },
        SliceAction::Outcome { action } => match action {
            OutcomeAction::Set { name, phase, kind } => outcome_set(ctx, name, phase, kind),
            OutcomeAction::Show { name } => outcome_show(ctx, name),
        },
        SliceAction::Journal { action } => match action {
            JournalAction::Append {
                name,
                phase,
                kind,
                summary,
                context,
            } => journal_append(ctx, name, phase, kind, summary, context),
            JournalAction::Show { name } => journal_show(ctx, name),
        },
        SliceAction::Transition { name, target } => transition(ctx, name, target),
        SliceAction::TouchedSpecs { name, scan, set } => touched_specs(ctx, name, scan, set),
        SliceAction::Overlap { name } => overlap(ctx, name),
        SliceAction::Archive { name } => archive(ctx, name),
        SliceAction::Drop { name, reason } => drop_slice(ctx, name, reason),
    }
}

fn create(
    ctx: &CommandContext, name: String, capability: Option<String>, if_exists: CreateIfExists,
) -> Result<CliResult, Error> {
    let capability_value = capability.map_or_else(
        || {
            ctx.config.capability.clone().ok_or_else(|| {
                Error::Config(
                    "no project capability declared; pass `--capability <id>` explicitly or run \
                     `specify init <capability>` first (hub projects cannot create changes)"
                        .to_string(),
                )
            })
        },
        Ok,
    )?;
    let slices_dir = ctx.slices_dir();
    std::fs::create_dir_all(&slices_dir)?;

    let outcome =
        slice_actions::create(&slices_dir, &name, &capability_value, if_exists, Utc::now())?;

    emit_slice_create(ctx.format, &outcome)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    name: String,
    slice_dir: String,
    status: String,
    capability: String,
    created: bool,
    restarted: bool,
}

fn emit_slice_create(format: OutputFormat, outcome: &CreateOutcome) -> Result<CliResult, Error> {
    match format {
        OutputFormat::Json => emit_response(CreateBody {
            name: outcome.dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            slice_dir: outcome.dir.display().to_string(),
            status: outcome.metadata.status.to_string(),
            capability: outcome.metadata.capability.clone(),
            created: outcome.created,
            restarted: outcome.restarted,
        })?,
        OutputFormat::Text => {
            if outcome.created {
                println!("Created slice {}", outcome.dir.display());
            } else {
                println!("Reusing existing slice {}", outcome.dir.display());
            }
            if outcome.restarted {
                println!("  (previous directory was removed)");
            }
            println!("  capability: {}", outcome.metadata.capability);
            println!("  status: {}", outcome.metadata.status);
        }
    }
    Ok(CliResult::Success)
}

fn transition(
    ctx: &CommandContext, name: String, target: LifecycleStatus,
) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let metadata = slice_actions::transition(&slice_dir, target, Utc::now())?;

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
        })?,
        OutputFormat::Text => {
            println!("{name}: status = {}", metadata.status);
        }
    }
    Ok(CliResult::Success)
}

fn touched_specs(
    ctx: &CommandContext, name: String, scan: bool, set: Vec<String>,
) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);

    let entries = if !set.is_empty() {
        let v = parse_touched_spec_set(&set)?;
        let metadata = slice_actions::write_touched(&slice_dir, v)?;
        metadata.touched_specs
    } else if scan {
        // The scan classifies a delta as `new` vs `modified` against
        // the omnia ThreeWayMerge baseline. Reach through the omnia
        // synthesiser so any future change to the baseline location
        // (Phase 4.1) flows through one place.
        let classes = artifact_classes(&ctx.project_dir, &slice_dir);
        let baseline_dir = classes
            .iter()
            .find(|c| matches!(c.strategy, MergeStrategy::ThreeWayMerge))
            .map_or_else(
                || ProjectConfig::specify_dir(&ctx.project_dir).join("specs"),
                |c| c.baseline_dir.clone(),
            );
        let scanned = slice_actions::scan_touched(&slice_dir, &baseline_dir)?;
        let metadata = slice_actions::write_touched(&slice_dir, scanned)?;
        metadata.touched_specs
    } else {
        let metadata = SliceMetadata::load(&slice_dir)?;
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
        })?,
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
            "new" => SpecKind::New,
            "modified" => SpecKind::Modified,
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

fn overlap(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slices_dir = ctx.slices_dir();
    let overlaps = slice_actions::overlap(&slices_dir, &name)?;

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
                    other_slice: o.other.clone(),
                    our_spec_type: o.ours.to_string(),
                    other_spec_type: o.theirs.to_string(),
                })
                .collect(),
        })?,
        OutputFormat::Text => {
            if overlaps.is_empty() {
                println!("{name}: no overlapping slices");
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

fn archive(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let target = slice_actions::archive(&slice_dir, &archive_dir, Utc::now())?;

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
        })?,
        OutputFormat::Text => {
            println!("{name}: archived to {}", target.display());
        }
    }
    Ok(CliResult::Success)
}

fn drop_slice(
    ctx: &CommandContext, name: String, reason: Option<String>,
) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let (metadata, archive_path) =
        slice_actions::drop(&slice_dir, &archive_dir, reason.as_deref(), Utc::now())?;

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
        })?,
        OutputFormat::Text => {
            println!("{name}: dropped and archived to {}", archive_path.display());
            if let Some(r) = &metadata.drop_reason {
                println!("  reason: {r}");
            }
        }
    }
    Ok(CliResult::Success)
}

fn outcome_set(
    ctx: &CommandContext, name: String, phase: Phase, kind: OutcomeKindAction,
) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let (outcome, summary, context) = build_outcome(kind);

    let metadata = slice_actions::stamp_outcome(
        &slice_dir,
        phase,
        outcome.clone(),
        &summary,
        context.as_deref(),
        Utc::now(),
    )?;

    let stamped = metadata
        .outcome
        .as_ref()
        .expect("stamp_outcome action must set metadata.outcome on success");
    let outcome_str = outcome.discriminant();
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PhaseStamp {
        slice: String,
        phase: String,
        outcome: String,
        at: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PhaseStamp {
            slice: name,
            phase: phase.to_string(),
            outcome: outcome_str.to_string(),
            at: stamped.at.to_string(),
        })?,
        OutputFormat::Text => {
            println!("Stamped outcome '{outcome_str}' for phase '{phase}' on slice '{name}'.");
        }
    }
    Ok(CliResult::Success)
}

/// Lower a `slice outcome set` subcommand into the wire `Outcome`,
/// summary, and optional context. clap has already enforced
/// per-variant flag presence; no runtime guard required.
fn build_outcome(kind: OutcomeKindAction) -> (Outcome, String, Option<String>) {
    match kind {
        OutcomeKindAction::Success { summary, context } => (Outcome::Success, summary, context),
        OutcomeKindAction::Failure { summary, context } => (Outcome::Failure, summary, context),
        OutcomeKindAction::Deferred { summary, context } => (Outcome::Deferred, summary, context),
        OutcomeKindAction::RegistryAmendmentRequired {
            summary,
            context,
            proposed_name,
            proposed_url,
            proposed_capability,
            proposed_description,
            rationale,
        } => {
            let summary =
                summary.unwrap_or_else(|| format!("registry-amendment-required: {proposed_name}"));
            let outcome = Outcome::RegistryAmendmentRequired {
                proposed_name,
                proposed_url,
                proposed_capability,
                proposed_description,
                rationale,
            };
            (outcome, summary, context)
        }
    }
}

/// Report the stamped `.metadata.yaml.outcome` for `name`.
///
/// Symmetric with [`outcome_set`] (the writer): this is
/// the read verb `/change:execute` consumes after a phase returns.
/// Emits `"outcome": null` when the slice exists but nothing has
/// been stamped; exits `CliResult::Success` in both cases — an unstamped
/// slice is not an error, just an absence.
///
/// Falls back to `.specify/archive/` when the slice is not found under
/// `.specify/slices/`. This handles the post-merge case: `slice merge run`
/// stamps the outcome into `.metadata.yaml` and then archives the slice
/// directory, so the active path no longer exists. The fallback scans
/// archive entries matching `*-<name>` and picks the most recent by
/// `created-at` timestamp.
fn outcome_show(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let metadata = if slice_dir.is_dir() {
        SliceMetadata::load(&slice_dir)?
    } else {
        resolve_archived_metadata(&ctx.project_dir, &name)?
    };

    match ctx.format {
        OutputFormat::Json => emit_outcome_show_json(name, &metadata)?,
        OutputFormat::Text => match &metadata.outcome {
            Some(o) => {
                println!("{name}: {}/{} — {}", o.phase, o.outcome, o.summary);
                if let Outcome::RegistryAmendmentRequired {
                    proposed_name,
                    proposed_url,
                    proposed_capability,
                    proposed_description,
                    rationale,
                } = &o.outcome
                {
                    println!("  proposed-name: {proposed_name}");
                    println!("  proposed-url: {proposed_url}");
                    println!("  proposed-capability: {proposed_capability}");
                    if let Some(desc) = proposed_description {
                        println!("  proposed-description: {desc}");
                    }
                    println!("  rationale: {rationale}");
                }
            }
            None => {
                println!("{name}: no outcome stamped");
            }
        },
    }
    Ok(CliResult::Success)
}

/// JSON serialiser for `slice outcome show`.
///
/// Splitting the JSON branch out of [`outcome_show`] keeps
/// the dispatcher readable and lets the helper own the
/// `Outcome::RegistryAmendmentRequired` payload-extraction shim that
/// RFC-9 §2B introduces. The on-disk wire (in `.metadata.yaml`)
/// nests the proposal under `outcome.outcome.registry-amendment-required.*`;
/// the CLI shape is flatter — `outcome.outcome` stays a kebab-case
/// string and the structured payload is hoisted into a sibling
/// `outcome.proposal` object so existing consumers that only read
/// `.outcome.outcome` (the historical contract `/change:execute` pins)
/// keep working unchanged.
fn emit_outcome_show_json(name: String, metadata: &SliceMetadata) -> Result<(), Error> {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        proposal: Option<RegistryProposalRow>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct RegistryProposalRow {
        proposed_name: String,
        proposed_url: String,
        proposed_capability: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        proposed_description: Option<String>,
        rationale: String,
    }
    fn proposal_for(outcome: &Outcome) -> Option<RegistryProposalRow> {
        if let Outcome::RegistryAmendmentRequired {
            proposed_name,
            proposed_url,
            proposed_capability,
            proposed_description,
            rationale,
        } = outcome
        {
            Some(RegistryProposalRow {
                proposed_name: proposed_name.clone(),
                proposed_url: proposed_url.clone(),
                proposed_capability: proposed_capability.clone(),
                proposed_description: proposed_description.clone(),
                rationale: rationale.clone(),
            })
        } else {
            None
        }
    }
    let outcome_detail = metadata.outcome.as_ref().map(|o| OutcomeRow {
        phase: o.phase.to_string(),
        outcome: o.outcome.discriminant().to_string(),
        at: o.at.clone(),
        summary: o.summary.clone(),
        context: o.context.clone().map_or(Value::Null, Value::from),
        proposal: proposal_for(&o.outcome),
    });
    emit_response(OutcomeResponse {
        name,
        outcome: outcome_detail,
    })?;
    Ok(())
}

/// Scan `.specify/archive/` for directories whose name ends with
/// `-<slice_name>` (the `YYYY-MM-DD-<name>` convention), load each
/// candidate's `.metadata.yaml`, and return the most recent by
/// `created-at`. Used by `outcome_show` as a fallback when the
/// active slice directory has been archived by `slice merge run`.
fn resolve_archived_metadata(project_dir: &Path, slice_name: &str) -> Result<SliceMetadata, Error> {
    let archive_dir = ProjectConfig::archive_dir(project_dir);
    let suffix = format!("-{slice_name}");
    let mut candidates: Vec<(String, SliceMetadata)> = Vec::new();

    if archive_dir.is_dir() {
        let entries = std::fs::read_dir(&archive_dir)?;
        for entry in entries {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(&suffix) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if let Ok(meta) = SliceMetadata::load(&entry.path()) {
                let created = meta.created_at.as_deref().unwrap_or("").to_string();
                candidates.push((created.clone(), meta));
            }
        }
    }

    if candidates.is_empty() {
        return Err(Error::SliceNotFound {
            name: slice_name.to_string(),
        });
    }

    let (_, metadata) = candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .expect("candidates is non-empty (checked above)");
    Ok(metadata)
}

fn journal_append(
    ctx: &CommandContext, name: String, phase: Phase, kind: EntryKind, summary: String,
    context: Option<String>,
) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let timestamp = format_rfc3339(Utc::now());
    let entry = JournalEntry {
        timestamp: timestamp.clone(),
        step: phase,
        r#type: kind,
        summary,
        context,
    };

    Journal::append(&slice_dir, entry)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct JournalBody {
        slice: String,
        phase: String,
        kind: String,
        timestamp: Rfc3339Stamp,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(JournalBody {
            slice: name,
            phase: phase.to_string(),
            kind: kind.to_string(),
            timestamp,
        })?,
        OutputFormat::Text => {
            println!("Appended {kind} entry to {name}/journal.yaml.");
        }
    }
    Ok(CliResult::Success)
}

fn journal_show(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let journal = Journal::load(&slice_dir)?;
    journal_show::emit(ctx.format, &name, &journal)?;
    Ok(CliResult::Success)
}

mod journal_show {
    use serde::Serialize;
    use serde_json::Value;
    use specify::{Error, Journal, JournalEntry, Rfc3339Stamp};

    use crate::cli::OutputFormat;
    use crate::output::emit_response;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct Body {
        name: String,
        entries: Vec<EntryRow>,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct EntryRow {
        timestamp: Rfc3339Stamp,
        phase: String,
        kind: String,
        summary: String,
        context: Value,
    }

    pub(super) fn emit(format: OutputFormat, name: &str, journal: &Journal) -> Result<(), Error> {
        match format {
            OutputFormat::Json => emit_response(Body {
                name: name.to_string(),
                entries: journal.entries.iter().map(entry_row).collect(),
            })?,
            OutputFormat::Text => print_text(name, journal),
        }
        Ok(())
    }

    fn entry_row(entry: &JournalEntry) -> EntryRow {
        EntryRow {
            timestamp: entry.timestamp.clone(),
            phase: entry.step.to_string(),
            kind: entry.r#type.to_string(),
            summary: entry.summary.clone(),
            context: entry.context.clone().map_or(Value::Null, Value::from),
        }
    }

    fn print_text(name: &str, journal: &Journal) {
        if journal.entries.is_empty() {
            println!("{name}: no journal entries");
            return;
        }

        println!("{name}:");
        for entry in &journal.entries {
            println!("  [{}] {}/{} — {}", entry.timestamp, entry.step, entry.r#type, entry.summary);
            if let Some(context) = &entry.context {
                for line in context.lines() {
                    println!("      {line}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// slice validate
// ---------------------------------------------------------------------------

fn validate(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let pipeline = ctx.load_pipeline()?;
    let report = validate_slice(&slice_dir, &pipeline)?;

    match ctx.format {
        OutputFormat::Json => emit_response(serialize_report(&report))?,
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
// slice merge run / preview / conflict-check
// ---------------------------------------------------------------------------

/// RFC-3b: Detect whether a project directory is inside a workspace clone.
/// Two-part heuristic: (1) the path contains `/.specify/workspace/*/` as an
/// ancestor via structural component walk, and (2) `.specify/project.yaml`
/// exists in the project directory. The secondary guard — CWD does not
/// contain `.specify/plan.yaml` — is retained as a safety check but is not
/// sufficient on its own because `plan.yaml` may be absent after
/// `specify change plan archive`.
fn is_workspace_clone(project_dir: &Path) -> bool {
    if !is_workspace_clone_path(project_dir) {
        return false;
    }
    let has_project_yaml = project_dir.join(".specify").join("project.yaml").exists();
    let has_plan_yaml = ProjectConfig::plan_path(project_dir).exists();
    has_project_yaml && !has_plan_yaml
}

fn merge_pathspecs(project_dir: &Path) -> Vec<&'static str> {
    WORKSPACE_MERGE_COMMIT_PATHS
        .iter()
        .copied()
        .filter(|path| project_dir.join(path).exists())
        .collect()
}

fn git(project_dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("git").arg("-C").arg(project_dir).args(args).output()
}

fn auto_commit_merge(project_dir: &Path, name: &str) {
    let pathspecs = merge_pathspecs(project_dir);
    if pathspecs.is_empty() {
        return;
    }
    let warn = |step: &str, msg: &str| eprintln!("warning: workspace auto-commit {step}: {msg}");
    let run = |step: &str, args: &[&str]| -> Option<std::process::Output> {
        match git(project_dir, args) {
            Ok(output) => Some(output),
            Err(err) => {
                warn(step, &err.to_string());
                None
            }
        }
    };

    let mut add_args = vec!["add", "--"];
    add_args.extend(pathspecs.iter().copied());
    let Some(add) = run("git-add", &add_args) else { return };
    if !add.status.success() {
        warn("git-add", &String::from_utf8_lossy(&add.stderr));
        return;
    }

    let mut diff_args = vec!["diff", "--cached", "--quiet", "--"];
    diff_args.extend(pathspecs.iter().copied());
    match git(project_dir, &diff_args).map(|o| o.status) {
        Ok(status) if status.success() => return,
        Ok(status) if status.code() == Some(1) => {}
        Ok(status) => return warn("diff check", &format!("status {status}")),
        Err(err) => return warn("diff check", &err.to_string()),
    }

    let commit_msg = format!("specify: merge {name}");
    let mut commit_args = vec!["commit", "-m", &commit_msg, "--"];
    commit_args.extend(pathspecs.iter().copied());
    if let Some(commit) = run("commit", &commit_args)
        && !commit.status.success()
    {
        warn("commit", &String::from_utf8_lossy(&commit.stderr));
    }
}

fn merge_run(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);

    let merged = merge_slice(&slice_dir, &classes, &archive_dir)?;

    // RFC-14 C05: the merge-owned workspace commit is limited to the
    // baseline spec tree and archived slice. Opaque/generated outputs
    // remain as residue for the execute driver.
    if is_workspace_clone(&ctx.project_dir) {
        auto_commit_merge(&ctx.project_dir, &name);
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
            let specs: Vec<Value> = merged.iter().map(entry_json).collect();
            emit_response(MergeResponse { merged_specs: specs })?;
        }
        OutputFormat::Text => {
            for entry in &merged {
                println!("{}: {}", entry.name, summarise_ops(&entry.result.operations));
            }
            println!("Archived to {}", archive_path.display());
        }
    }
    Ok(CliResult::Success)
}

fn merge_preview(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);
    let result = preview_slice(&slice_dir, &classes)?;

    // The JSON preview surface keeps its pre-2.8 shape — `specs` and
    // `contracts` arrays — by grouping the engine's class-tagged
    // entries by their `class_name`. The literal output keys live
    // here, alongside the omnia-default synthesiser, rather than in
    // the engine.
    let specs_entries: Vec<&MergePreviewEntry> =
        result.three_way.iter().filter(|e| e.class_name == "specs").collect();
    let contract_entries: Vec<&OpaquePreviewEntry> =
        result.opaque.iter().filter(|e| e.class_name == "contracts").collect();

    match ctx.format {
        OutputFormat::Json => {
            let specs: Vec<Value> =
                specs_entries.iter().map(|e| preview_entry_to_json(e)).collect();
            let contracts: Vec<Value> =
                contract_entries.iter().map(|c| contract_to_json(c)).collect();
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PreviewBody {
                slice_dir: String,
                specs: Vec<Value>,
                contracts: Vec<Value>,
            }
            emit_response(PreviewBody {
                slice_dir: slice_dir.display().to_string(),
                specs,
                contracts,
            })?;
        }
        OutputFormat::Text => {
            if specs_entries.is_empty() {
                println!("No delta specs to merge.");
            } else {
                for entry in &specs_entries {
                    println!("{}: {}", entry.name, summarise_ops(&entry.result.operations));
                    for op in &entry.result.operations {
                        println!("  {}", operation_label(op));
                    }
                }
            }
            if !contract_entries.is_empty() {
                println!("\nContract changes:");
                for c in &contract_entries {
                    let (sigil, label) = match c.action {
                        OpaqueAction::Added => ("+", "added"),
                        OpaqueAction::Replaced => ("~", "replaced"),
                        _ => ("?", "unknown"),
                    };
                    println!("  {sigil} contracts/{} ({label})", c.relative_path);
                }
            }
        }
    }
    Ok(CliResult::Success)
}

fn merge_conflict_check(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let classes = artifact_classes(&ctx.project_dir, &slice_dir);
    let conflicts = conflict_check(&slice_dir, &classes)?;

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ConflictCheckResponse {
                slice_dir: String,
                conflicts: Vec<Value>,
            }
            let items: Vec<Value> = conflicts.iter().map(baseline_conflict_to_json).collect();
            emit_response(ConflictCheckResponse {
                slice_dir: slice_dir.display().to_string(),
                conflicts: items,
            })?;
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
struct MergePreviewEntryJson {
    name: String,
    operations: Vec<Value>,
}

fn entry_json(entry: &MergePreviewEntry) -> Value {
    let ops: Vec<Value> = entry.result.operations.iter().map(op_json).collect();
    serde_json::to_value(MergePreviewEntryJson {
        name: entry.name.clone(),
        operations: ops,
    })
    .expect("MergePreviewEntryJson serialises")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SpecPreviewEntryJson {
    name: String,
    baseline_path: String,
    operations: Vec<Value>,
}

fn preview_entry_to_json(entry: &MergePreviewEntry) -> Value {
    let ops: Vec<Value> = entry.result.operations.iter().map(op_json).collect();
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

fn contract_to_json(entry: &OpaquePreviewEntry) -> Value {
    let action = match entry.action {
        OpaqueAction::Added => "added",
        OpaqueAction::Replaced => "replaced",
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
#[serde(tag = "kind", rename_all = "kebab-case")]
enum MergeOpJson {
    Added { id: String, name: String },
    Modified { id: String, name: String },
    Removed { id: String, name: String },
    Renamed { id: String, old_name: String, new_name: String },
    CreatedBaseline { requirement_count: usize },
}

fn op_json(op: &MergeOperation) -> Value {
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

fn summarise_ops(ops: &[MergeOperation]) -> String {
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
// slice task (progress / mark)
// ---------------------------------------------------------------------------

fn task_progress(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let tasks_path = resolve_tasks_path(&ctx.project_dir, &slice_dir)?;
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
            })?;
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

fn task_mark(ctx: &CommandContext, name: String, task_number: String) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);
    let tasks_path = resolve_tasks_path(&ctx.project_dir, &slice_dir)?;
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
            })?;
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

/// Resolve the `tasks.md` path for a slice.
///
/// Walks the pipeline view to find the `build` brief's `tracks` value
/// (the id of the tasks brief), then uses that brief's `generates`
/// field as the relative path under `slice_dir`. This lets the CLI
/// honour schemas that rename `tasks.md` or nest it elsewhere.
fn resolve_tasks_path(project_dir: &Path, slice_dir: &Path) -> Result<PathBuf, Error> {
    let metadata = SliceMetadata::load(slice_dir)?;
    resolve_tasks_path_for(slice_dir, &metadata.capability, Some(project_dir))
}

pub fn resolve_tasks_path_for(
    slice_dir: &Path, capability_value: &str, project_hint: Option<&Path>,
) -> Result<PathBuf, Error> {
    // Use the hinted project dir when supplied; otherwise walk up from
    // the slice dir — convention is `<project>/.specify/slices/<name>`.
    let project_dir = match project_hint {
        Some(p) => p.to_path_buf(),
        None => slice_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                Error::Config(format!(
                    "cannot resolve project root from slice dir {}",
                    slice_dir.display()
                ))
            })?,
    };
    let pipeline = PipelineView::load(capability_value, &project_dir)?;
    let build_brief = pipeline
        .brief("build")
        .ok_or_else(|| Error::Config("capability has no `build` brief".to_string()))?;
    let tracks_id = build_brief
        .frontmatter
        .tracks
        .as_deref()
        .ok_or_else(|| Error::Config("`build` brief has no `tracks` field".to_string()))?;
    let tracked = pipeline.brief(tracks_id).ok_or_else(|| {
        Error::Config(format!("`build.tracks = {tracks_id}` but no such brief exists"))
    })?;
    let generates = brief_generates(tracked)?;
    Ok(slice_dir.join(generates))
}

fn brief_generates(brief: &Brief) -> Result<&str, Error> {
    brief.frontmatter.generates.as_deref().ok_or_else(|| {
        Error::Config(format!("brief `{}` has no `generates` field", brief.frontmatter.id))
    })
}

// ---------------------------------------------------------------------------
// slice list / slice status (multi-slice list + single-slice view)
// ---------------------------------------------------------------------------

pub(super) struct StatusEntry {
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

pub(super) fn status_entry_to_json(e: &StatusEntry) -> Value {
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

pub(super) fn collect_status(
    slice_dir: &Path, name: &str, pipeline: &PipelineView, project_dir: &Path,
) -> Result<StatusEntry, Error> {
    let metadata = SliceMetadata::load(slice_dir)?;
    let status_str = metadata.status.to_string();

    // Delegate per-brief artifact completion to `PipelineView` so every
    // consumer — `specify status`, `specify schema pipeline`, and any
    // future skill callers — agrees on what "complete" means.
    let artifacts = pipeline.completion_for(Phase::Define, slice_dir);

    let tasks = match resolve_tasks_path_for(slice_dir, &metadata.capability, Some(project_dir)) {
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

pub(super) fn list_slice_names(slices_dir: &Path) -> Result<Vec<String>, Error> {
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

fn list(ctx: &CommandContext) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let slices_dir = ctx.slices_dir();
    let names = list_slice_names(&slices_dir)?;

    let mut entries: Vec<StatusEntry> = Vec::with_capacity(names.len());
    for name in names {
        let dir = slices_dir.join(&name);
        let entry = collect_status(&dir, &name, &pipeline, &ctx.project_dir)?;
        entries.push(entry);
    }

    emit_slice_list(ctx.format, &entries)?;
    Ok(CliResult::Success)
}

fn status_one(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;
    let slice_dir = ctx.slices_dir().join(&name);
    let entry = collect_status(&slice_dir, &name, &pipeline, &ctx.project_dir)?;

    emit_slice_list(ctx.format, std::slice::from_ref(&entry))?;
    Ok(CliResult::Success)
}

fn emit_slice_list(format: OutputFormat, entries: &[StatusEntry]) -> Result<(), Error> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct StatusResponse {
                slices: Vec<Value>,
            }
            let slices: Vec<Value> = entries.iter().map(status_entry_to_json).collect();
            emit_response(StatusResponse { slices })?;
        }
        OutputFormat::Text => print_slice_list_text(entries),
    }
    Ok(())
}

fn print_slice_list_text(entries: &[StatusEntry]) {
    if entries.is_empty() {
        println!("No slices.");
        return;
    }
    if entries.len() == 1 {
        let e = &entries[0];
        println!("{}", e.name);
        println!("  capability: {}", e.capability);
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
        "slice",
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
    other_slice: String,
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
