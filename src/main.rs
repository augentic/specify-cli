//! `specify` binary entry point.
//!
//! The binary is a thin dispatcher over the library: it parses CLI
//! arguments via `clap`, loads `.specify/project.yaml` (which transitively
//! enforces the `specify_version` floor), runs the subcommand, and maps
//! any error onto the exit-code contract below.
//!
//! # Exit codes — documented contract for skill authors
//!
//! - `0` ([`EXIT_SUCCESS`]): Success.
//! - `1` ([`EXIT_GENERIC_FAILURE`]): Generic failure (I/O, parse,
//!   unknown).
//! - `2` ([`EXIT_VALIDATION_FAILED`]): Validation failed —
//!   `specify validate` returned a report whose `passed` flag is `false`.
//! - `3` ([`EXIT_VERSION_TOO_OLD`]): The CLI binary is older than the
//!   `specify_version` floor in `.specify/project.yaml`.
//!
//! Error → exit code mapping:
//! - [`Error::SpecifyVersionTooOld`] → `3`.
//! - [`Error::Validation`] → `2`.
//! - Any other [`Error`] variant → `1`.
//! - A successful `Commands::Validate` where `report.passed == false` →
//!   `2` (even though no `Error` is produced).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::Utc;
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use specify::{
    BaselineConflict, Brief, ChangeMetadata, CreateIfExists, CreateOutcome, EntryKind, Error,
    InitOptions, InitResult, Journal, JournalEntry, LifecycleStatus, MergeEntry, MergeOperation,
    MergeResult, Outcome, Overlap, Phase, PipelineView, Plan, PlanChange, PlanChangePatch,
    PlanLockAcquired, PlanLockReleased, PlanLockStamp, PlanLockState, PlanStatus,
    PlanValidationLevel, PlanValidationResult, ProjectConfig, Schema, SchemaSource, SpecType, Task,
    TouchedSpec, ValidationReport, ValidationResult, VersionMode, change_actions, conflict_check,
    format_rfc3339, init, mark_complete, merge_change, parse_tasks, preview_change,
    serialize_report, validate_change,
};

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_GENERIC_FAILURE: i32 = 1;
pub const EXIT_VALIDATION_FAILED: i32 = 2;
pub const EXIT_VERSION_TOO_OLD: i32 = 3;

/// JSON contract version emitted on every structured response. Bumping
/// this field is a breaking change for skill authors — see RFC-1
/// §"JSON Contract Versioning".
///
/// # v1 → v2 diff (RFC-2 §2)
///
/// - Every JSON key is now kebab-case. `schema_version` → `schema-version`,
///   `change_dir` → `change-dir`, `defined_at` → `defined-at`, and so on for
///   every snake-case key that was ever emitted by the CLI (see RFC-2 §2.1
///   for the full rename table). Library-derived types were already kebab
///   via `#[serde(rename_all = "kebab-case")]`; v2 aligns the hand-built
///   `json!({...})` blocks in `src/main.rs` and the
///   `specify-validate::serialize_report` helper with the same rule.
/// - New read verb `specify change outcome <name>` (added in RFC-2 §1.1 /
///   L0.A1) shipped under the v2 contract.
/// - Error variant identifiers surfaced as the `"error"` value in failure
///   payloads are kebab-case too: `not_initialized` → `not-initialized`,
///   `schema_resolution` → `schema-resolution`, `specify_version_too_old`
///   → `specify-version-too-old`, `plan_transition` → `plan-transition`,
///   `plan_has_outstanding_work` → `plan-has-outstanding-work`, and
///   `driver_busy` → `driver-busy`. Single-word variants (`io`, `yaml`,
///   `config`, `merge`, `lifecycle`, `validation`) were already kebab-safe
///   and are unchanged.
/// - No shape changes beyond the casing: key sets, nesting, and value
///   types are frozen.
const JSON_SCHEMA_VERSION: u64 = 2;

#[derive(Parser)]
#[command(
    name = "specify",
    version,
    about = "Specify CLI — deterministic operations for spec-driven development"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format
    #[arg(long, default_value = "text", global = true)]
    format: OutputFormat,
}

#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize .specify/ in a project
    Init {
        /// Schema name or URL
        schema: String,
        /// Schema source directory (pre-resolved)
        #[arg(long)]
        schema_dir: PathBuf,
        /// Project name (defaults to the project directory name)
        #[arg(long)]
        name: Option<String>,
        /// Project domain description (tech stack, architecture, testing)
        #[arg(long)]
        domain: Option<String>,
        /// Rewrite `specify_version` in `project.yaml` to the running
        /// binary's version. Used to bump the CLI floor after a
        /// user-driven upgrade.
        #[arg(long)]
        upgrade: bool,
    },

    /// Validate change artifacts against schema rules
    Validate {
        /// Change directory (.specify/changes/<name>)
        change_dir: PathBuf,
    },

    /// Merge all delta specs for a change into baseline and archive the change
    Merge {
        /// Change directory
        change_dir: PathBuf,
    },

    /// Show change status and task progress
    Status {
        /// Specific change name (optional)
        change: Option<String>,
    },

    /// Task operations
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },

    /// Schema operations
    Schema {
        #[command(subcommand)]
        action: SchemaAction,
    },

    /// Change lifecycle operations
    Change {
        #[command(subcommand)]
        action: ChangeAction,
    },

    /// Spec-level helpers (preview + conflict-check) that complement `merge`
    Spec {
        #[command(subcommand)]
        action: SpecAction,
    },

    /// Manage the initiative-level plan at `.specify/plan.yaml`
    Initiative {
        #[command(subcommand)]
        action: InitiativeAction,
    },

    /// Bootstrap and verify Crux cross-platform projects (RFC-6).
    ///
    /// The four verbs route to handlers in the `specify-vectis` library
    /// crate. They reuse the global `--format text|json` flag: JSON
    /// responses follow the v2 contract (kebab-case keys, auto-injected
    /// `schema-version: 2`, kebab-case error variants); text responses
    /// are humanised per-verb summaries.
    ///
    /// Exit codes reuse the binary's contract: missing prerequisites
    /// reports back as [`EXIT_VALIDATION_FAILED`] (`2`) — locally
    /// "your workstation is incomplete", which slots cleanly into the
    /// existing "validation failed" bucket — and every other failure
    /// returns [`EXIT_GENERIC_FAILURE`] (`1`).
    Vectis {
        #[command(subcommand)]
        action: VectisAction,
    },
}

/// Subcommands under `specify vectis`. Each variant flattens the
/// matching `clap::Args` struct from the `specify-vectis` library so
/// flag parsing stays in lock-step with the library definition.
#[derive(Subcommand)]
enum VectisAction {
    /// Scaffold a new Crux project (core + optional shells).
    Init(specify_vectis::InitArgs),
    /// Verify that a Crux project still builds end-to-end.
    Verify(specify_vectis::VerifyArgs),
    /// Add an iOS or Android shell to an existing core.
    AddShell(specify_vectis::AddShellArgs),
    /// Refresh pinned tool/crate versions and (optionally) verify them.
    UpdateVersions(specify_vectis::UpdateVersionsArgs),
}

#[derive(Subcommand)]
enum InitiativeAction {
    /// Scaffold an empty .specify/plan.yaml
    Init {
        /// Kebab-case initiative name
        name: String,
        /// Named source, repeated: --source <key>=<path-or-url>
        #[arg(long = "source", value_parser = parse_source_kv)]
        sources: Vec<(String, String)>,
    },
    /// Validate .specify/plan.yaml (structure + plan/change consistency)
    Validate,
    /// Return the next eligible plan entry (respects depends-on + in-progress)
    Next,
    /// Show initiative progress report
    Status,
    /// Add a new change entry (status: pending)
    Create {
        /// Kebab-case change name
        name: String,
        /// Ordering dependencies (repeatable). Every value is a change name in the plan.
        /// Pass `--depends-on` (with no value) to clear the field; omit the flag to
        /// leave it unchanged.
        #[arg(long = "depends-on", action = ArgAction::Append)]
        depends_on: Vec<String>,
        /// Impact annotations (repeatable). Every value is a change name in the plan.
        #[arg(long, action = ArgAction::Append)]
        affects: Vec<String>,
        /// Named source keys (repeatable). Every value is a key in the top-level
        /// `sources` map.
        #[arg(long = "sources", action = ArgAction::Append)]
        sources: Vec<String>,
        /// Free-text scoping hint for the define step
        #[arg(long)]
        description: Option<String>,
    },
    /// Edit non-status fields on an existing plan entry
    Amend {
        /// Kebab-case change name
        name: String,
        /// Replace depends-on. Pass `--depends-on` (with no value) to clear the
        /// field; omit the flag to leave it unchanged. Repeat or comma-separate
        /// to supply multiple values.
        #[arg(long = "depends-on", num_args = 0.., value_delimiter = ',')]
        depends_on: Option<Vec<String>>,
        /// Replace affects. Pass `--affects` (with no value) to clear the field;
        /// omit the flag to leave it unchanged.
        #[arg(long, num_args = 0.., value_delimiter = ',')]
        affects: Option<Vec<String>>,
        /// Replace sources. Pass `--sources` (with no value) to clear the field;
        /// omit the flag to leave it unchanged.
        #[arg(long = "sources", num_args = 0.., value_delimiter = ',')]
        sources: Option<Vec<String>>,
        /// Replace description. Pass `--description ""` to clear; omit the flag
        /// to leave it unchanged.
        #[arg(long)]
        description: Option<String>,
    },
    /// Apply a validated status transition
    Transition {
        /// Kebab-case change name
        name: String,
        /// Target status
        #[arg(value_enum)]
        target: PlanStatus,
        /// Free-text reason; only valid when transitioning to `failed`,
        /// `blocked`, or `skipped`.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Archive the current plan to .specify/archive/plans/<name>-<YYYYMMDD>.yaml
    Archive {
        /// Archive even when the plan has pending/in-progress/blocked/failed entries.
        /// Without --force, these outstanding statuses block the archive.
        #[arg(long)]
        force: bool,
    },
    /// Driver-lock primitives used by `/spec:execute` (advisory PID stamp).
    ///
    /// These verbs manage the `.specify/plan.lock` PID stamp that keeps two
    /// concurrent `/spec:execute` drivers from racing on `get next change`
    /// and plan transitions. See RFC-2 §"Driver Concurrency".
    Lock {
        #[command(subcommand)]
        action: LockAction,
    },
}

#[derive(Subcommand)]
enum LockAction {
    /// Acquire the plan.lock PID stamp.
    ///
    /// Fails with `Error::DriverBusy` when another live PID holds it.
    /// Stale stamps (dead PID / malformed contents) are reclaimed
    /// silently.
    Acquire {
        /// PID to stamp into the lock file. Defaults to `std::process::id()`
        /// of the `specify` binary. `/spec:execute` passes a stable
        /// agent-session PID so release can authenticate the holder.
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Release the stamp when we hold it.
    ///
    /// No-op when the file is absent. Refuses to clobber a stamp held
    /// by a different PID (stale-lock reclaim is the job of the L2.G
    /// self-heal path, not of release).
    Release {
        /// PID that expects to own the stamp. Defaults to
        /// `std::process::id()`.
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Report the current lock state (holder PID, stale flag).
    Status,
}

#[derive(Subcommand)]
enum SpecAction {
    /// Show the merge operations that would be applied, without writing
    Preview {
        /// Change directory
        change_dir: PathBuf,
    },
    /// Report `type: modified` baselines modified after this change's `defined_at`
    ConflictCheck {
        /// Change directory
        change_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    /// Report task completion counts (total, complete, pending)
    Progress { change_dir: PathBuf },
    /// Mark a task complete (idempotent — no-op if already complete)
    Mark { change_dir: PathBuf, task_number: String },
}

#[derive(Subcommand)]
enum SchemaAction {
    /// Resolve a schema value to a directory path
    Resolve {
        schema_value: String,
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
    /// Validate a schema.yaml file
    Check { schema_dir: PathBuf },
    /// List the briefs for a phase in topological order (optionally
    /// with completion status against a specific change)
    Pipeline {
        /// Pipeline phase to enumerate
        #[arg(value_enum)]
        phase: Phase,
        /// Change directory; when supplied, each brief includes a
        /// `present` boolean reflecting whether its `generates`
        /// artifact exists under the directory
        #[arg(long)]
        change: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ChangeAction {
    /// Create a new change directory with an initial `.metadata.yaml`
    Create {
        /// Kebab-case change name
        name: String,
        /// Schema identifier; defaults to the value in `.specify/project.yaml`
        #[arg(long)]
        schema: Option<String>,
        /// Behaviour when `<changes_dir>/<name>/` already exists
        #[arg(long, value_enum, default_value = "fail")]
        if_exists: CreateIfExistsArg,
    },
    /// List every active change under `.specify/changes/`
    List,
    /// Show the status of one change (alias of `specify status <name>`)
    Status {
        /// Change name (under `.specify/changes/`)
        name: String,
    },
    /// Transition a change to a new lifecycle status
    Transition {
        /// Change name
        name: String,
        /// Target status (`defined`, `building`, `complete`, `merged`, `dropped`, or `defining`)
        #[arg(value_enum)]
        target: LifecycleStatus,
    },
    /// Scan or overwrite `touched_specs` on `.metadata.yaml`
    TouchedSpecs {
        /// Change name
        name: String,
        /// Scan `specs/` subdirs and classify each as new or modified
        #[arg(long, conflicts_with = "set")]
        scan: bool,
        /// Replace `touched_specs` with the listed capabilities (each `<name>:new|modified`)
        #[arg(long, value_delimiter = ',')]
        set: Vec<String>,
    },
    /// Report overlapping `touched_specs` with other active changes
    Overlap {
        /// Change name
        name: String,
    },
    /// Archive a change directory into `.specify/archive/YYYY-MM-DD-<name>/`
    Archive {
        /// Change name
        name: String,
    },
    /// Transition a change to `dropped` and archive it
    Drop {
        /// Change name
        name: String,
        /// Free-text reason; surfaced in `.metadata.yaml.drop_reason` and the archive path
        #[arg(long)]
        reason: Option<String>,
    },
    /// Record the outcome of a phase (define|build|merge) on `.metadata.yaml`
    PhaseOutcome {
        /// Change name
        name: String,
        /// Phase this outcome applies to
        #[arg(value_enum)]
        phase: Phase,
        /// Outcome classification
        #[arg(value_enum)]
        outcome: Outcome,
        /// Short explanation of what happened (shown in plan status-reason on non-success)
        #[arg(long)]
        summary: String,
        /// Optional verbatim detail (stderr, ambiguous-requirement text, etc.)
        #[arg(long)]
        context: Option<String>,
    },
    /// Read the stamped `.metadata.yaml.outcome` for a change
    ///
    /// Symmetric read verb for `phase-outcome`: emits the current
    /// `outcome` subtree for consumers like `/spec:execute` that
    /// classify a phase return without needing the rest of the
    /// lifecycle-status payload. Exits 0 both when an outcome is
    /// present and when the change is unstamped (`outcome: null`).
    Outcome {
        /// Change name
        name: String,
    },
    /// Append an entry to the change's `journal.yaml`
    JournalAppend {
        /// Change name
        name: String,
        /// Phase that produced the entry
        #[arg(value_enum)]
        phase: Phase,
        /// Entry classification
        #[arg(value_enum)]
        kind: EntryKind,
        /// Short summary
        #[arg(long)]
        summary: String,
        /// Optional verbatim context (multi-line)
        #[arg(long)]
        context: Option<String>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum CreateIfExistsArg {
    /// Refuse when the directory exists (default)
    Fail,
    /// Reuse the existing directory — requires a valid `.metadata.yaml`
    Continue,
    /// Delete and recreate — destructive
    Restart,
}

impl From<CreateIfExistsArg> for CreateIfExists {
    fn from(value: CreateIfExistsArg) -> Self {
        match value {
            CreateIfExistsArg::Fail => CreateIfExists::Fail,
            CreateIfExistsArg::Continue => CreateIfExists::Continue,
            CreateIfExistsArg::Restart => CreateIfExists::Restart,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = run(cli);
    ExitCode::from(code as u8)
}

fn run(cli: Cli) -> i32 {
    match cli.command {
        Commands::Init {
            schema,
            schema_dir,
            name,
            domain,
            upgrade,
        } => run_init(cli.format, schema, schema_dir, name, domain, upgrade),
        Commands::Validate { change_dir } => run_validate(cli.format, change_dir),
        Commands::Merge { change_dir } => run_merge(cli.format, change_dir),
        Commands::Status { change } => run_status(cli.format, change),
        Commands::Task { action } => match action {
            TaskAction::Progress { change_dir } => run_task_progress(cli.format, change_dir),
            TaskAction::Mark {
                change_dir,
                task_number,
            } => run_task_mark(cli.format, change_dir, task_number),
        },
        Commands::Schema { action } => match action {
            SchemaAction::Resolve {
                schema_value,
                project_dir,
            } => run_schema_resolve(cli.format, schema_value, project_dir),
            SchemaAction::Check { schema_dir } => run_schema_check(cli.format, schema_dir),
            SchemaAction::Pipeline { phase, change } => {
                run_schema_pipeline(cli.format, phase, change)
            }
        },
        Commands::Change { action } => run_change(cli.format, action),
        Commands::Spec { action } => match action {
            SpecAction::Preview { change_dir } => run_spec_preview(cli.format, change_dir),
            SpecAction::ConflictCheck { change_dir } => {
                run_spec_conflict_check(cli.format, change_dir)
            }
        },
        Commands::Initiative { action } => run_initiative(cli.format, action),
        Commands::Vectis { action } => run_vectis(cli.format, &action),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn run_init(
    format: OutputFormat, schema: String, schema_dir: PathBuf, name: Option<String>,
    domain: Option<String>, upgrade: bool,
) -> i32 {
    // `upgrade` toggles future behaviour (Preserve vs WriteCurrent in
    // Change K), but for Change J both fresh and `--upgrade` write the
    // running binary's version. Accept the flag today so skills can
    // migrate to it without a CLI bump.
    let _ = upgrade;
    let project_dir = PathBuf::from(".");

    let opts = InitOptions {
        project_dir: &project_dir,
        schema_value: &schema,
        schema_source_dir: &schema_dir,
        name: name.as_deref(),
        domain: domain.as_deref(),
        version_mode: VersionMode::WriteCurrent,
    };

    match init(opts) {
        Ok(result) => emit_init_result(format, &result),
        Err(err) => emit_error(format, &err),
    }
}

fn emit_init_result(format: OutputFormat, result: &InitResult) -> i32 {
    match format {
        OutputFormat::Json => {
            let value = json!({
                "config-path": absolute_string(&result.config_path),
                "schema-name": result.schema_name,
                "cache-present": result.cache_present,
                "directories-created": result.directories_created
                    .iter()
                    .map(|p| absolute_string(p))
                    .collect::<Vec<_>>(),
                "scaffolded-rule-keys": result.scaffolded_rule_keys,
                "specify-version": result.specify_version,
            });
            emit_json(value);
        }
        OutputFormat::Text => {
            println!("Initialized .specify/");
            println!("  schema: {}", result.schema_name);
            println!("  config: {}", absolute_string(&result.config_path));
            println!("  cache present: {}", result.cache_present);
            if !result.directories_created.is_empty() {
                println!(
                    "  directories created: {}",
                    result
                        .directories_created
                        .iter()
                        .map(|p| absolute_string(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            println!("  specify_version: {}", result.specify_version);
        }
    }
    EXIT_SUCCESS
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

fn run_validate(format: OutputFormat, change_dir: PathBuf) -> i32 {
    let (project_dir, config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let pipeline = match PipelineView::load(&config.schema, &project_dir) {
        Ok(view) => view,
        Err(err) => return emit_error(format, &err),
    };
    let report = match validate_change(&change_dir, &pipeline) {
        Ok(report) => report,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => emit_json(serialize_report(&report)),
        OutputFormat::Text => print_validation_report_text(&report),
    }

    if report.passed { EXIT_SUCCESS } else { EXIT_VALIDATION_FAILED }
}

fn print_validation_report_text(report: &ValidationReport) {
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

fn format_result_line(r: &ValidationResult) -> String {
    match r {
        ValidationResult::Pass { rule_id, .. } => format!("[ok] {rule_id}"),
        ValidationResult::Fail { rule_id, detail, .. } => format!("[fail] {rule_id}: {detail}"),
        ValidationResult::Deferred { rule_id, reason, .. } => {
            format!("[defer] {rule_id} ({reason})")
        }
    }
}

// ---------------------------------------------------------------------------
// merge
// ---------------------------------------------------------------------------

fn run_merge(format: OutputFormat, change_dir: PathBuf) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let specs_dir = ProjectConfig::specs_dir(&project_dir);
    let archive_dir = ProjectConfig::archive_dir(&project_dir);

    // Capture the change basename before `merge_change` moves the
    // directory under archive/.
    let change_name = match change_dir.file_name().and_then(|s| s.to_str()) {
        Some(name) => name.to_string(),
        None => {
            let err =
                Error::Config(format!("change dir `{}` has no basename", change_dir.display()));
            return emit_error(format, &err);
        }
    };

    let merged = match merge_change(&change_dir, &specs_dir, &archive_dir) {
        Ok(m) => m,
        Err(err) => return emit_error(format, &err),
    };

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let archive_path = archive_dir.join(format!("{today}-{change_name}"));

    match format {
        OutputFormat::Json => {
            let specs: Vec<Value> = merged.iter().map(merge_entry_to_json).collect();
            emit_json(json!({
                "merged-specs": specs,
            }));
        }
        OutputFormat::Text => {
            for (name, result) in &merged {
                println!("{name}: {}", summarise_operations(&result.operations));
            }
            println!("Archived to {}", archive_path.display());
        }
    }
    EXIT_SUCCESS
}

fn merge_entry_to_json(entry: &(String, MergeResult)) -> Value {
    let (name, result) = entry;
    let ops: Vec<Value> = result.operations.iter().map(merge_op_to_json).collect();
    json!({
        "name": name,
        "operations": ops,
    })
}

// ---------------------------------------------------------------------------
// spec preview / conflict-check
// ---------------------------------------------------------------------------

fn run_spec_preview(format: OutputFormat, change_dir: PathBuf) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let specs_dir = ProjectConfig::specs_dir(&project_dir);
    let entries = match preview_change(&change_dir, &specs_dir) {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => {
            let specs: Vec<Value> = entries.iter().map(preview_entry_to_json).collect();
            emit_json(json!({
                "change-dir": change_dir.display().to_string(),
                "specs": specs,
            }));
        }
        OutputFormat::Text => {
            if entries.is_empty() {
                println!("No delta specs to merge.");
            } else {
                for entry in &entries {
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
        }
    }
    EXIT_SUCCESS
}

fn preview_entry_to_json(entry: &MergeEntry) -> Value {
    let ops: Vec<Value> = entry.result.operations.iter().map(merge_op_to_json).collect();
    json!({
        "name": entry.spec_name,
        "baseline-path": entry.baseline_path.display().to_string(),
        "operations": ops,
    })
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
    }
}

fn run_spec_conflict_check(format: OutputFormat, change_dir: PathBuf) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let specs_dir = ProjectConfig::specs_dir(&project_dir);
    let conflicts = match conflict_check(&change_dir, &specs_dir) {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => {
            let items: Vec<Value> = conflicts.iter().map(baseline_conflict_to_json).collect();
            emit_json(json!({
                "change-dir": change_dir.display().to_string(),
                "conflicts": items,
            }));
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
    EXIT_SUCCESS
}

fn baseline_conflict_to_json(c: &BaselineConflict) -> Value {
    json!({
        "capability": c.capability,
        "defined-at": c.defined_at,
        "baseline-modified-at": c.baseline_modified_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    })
}

fn merge_op_to_json(op: &MergeOperation) -> Value {
    match op {
        MergeOperation::Added { id, name } => json!({
            "kind": "added",
            "id": id,
            "name": name,
        }),
        MergeOperation::Modified { id, name } => json!({
            "kind": "modified",
            "id": id,
            "name": name,
        }),
        MergeOperation::Removed { id, name } => json!({
            "kind": "removed",
            "id": id,
            "name": name,
        }),
        MergeOperation::Renamed {
            id,
            old_name,
            new_name,
        } => json!({
            "kind": "renamed",
            "id": id,
            "old-name": old_name,
            "new-name": new_name,
        }),
        MergeOperation::CreatedBaseline { requirement_count } => json!({
            "kind": "created_baseline",
            "requirement-count": requirement_count,
        }),
    }
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
// status
// ---------------------------------------------------------------------------

fn run_status(format: OutputFormat, change: Option<String>) -> i32 {
    let (project_dir, config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let pipeline = match PipelineView::load(&config.schema, &project_dir) {
        Ok(view) => view,
        Err(err) => return emit_error(format, &err),
    };
    let changes_dir = ProjectConfig::changes_dir(&project_dir);

    let names: Vec<String> = match &change {
        Some(n) => vec![n.clone()],
        None => match list_change_names(&changes_dir) {
            Ok(v) => v,
            Err(err) => return emit_error(format, &err),
        },
    };

    let mut entries: Vec<StatusEntry> = Vec::new();
    for name in names {
        let dir = changes_dir.join(&name);
        let entry = match collect_status(&dir, &name, &pipeline, &project_dir) {
            Ok(entry) => entry,
            Err(err) => return emit_error(format, &err),
        };
        entries.push(entry);
    }

    match format {
        OutputFormat::Json => {
            let changes: Vec<Value> = entries.iter().map(status_entry_to_json).collect();
            emit_json(json!({ "changes": changes }));
        }
        OutputFormat::Text => print_status_text(&entries),
    }
    EXIT_SUCCESS
}

struct StatusEntry {
    name: String,
    schema: String,
    status: String,
    tasks: Option<(usize, usize)>,
    artifacts: std::collections::BTreeMap<String, bool>,
}

fn status_entry_to_json(e: &StatusEntry) -> Value {
    let tasks_value = match &e.tasks {
        Some((complete, total)) => json!({"total": total, "complete": complete}),
        None => Value::Null,
    };
    let artifacts: serde_json::Map<String, Value> =
        e.artifacts.iter().map(|(k, v)| (k.clone(), Value::Bool(*v))).collect();
    json!({
        "name": e.name,
        "status": e.status,
        "schema": e.schema,
        "tasks": tasks_value,
        "artifacts": Value::Object(artifacts),
    })
}

fn collect_status(
    change_dir: &Path, name: &str, pipeline: &PipelineView, project_dir: &Path,
) -> Result<StatusEntry, Error> {
    let metadata = ChangeMetadata::load(change_dir)?;
    let status_str = format!("{:?}", metadata.status).to_lowercase();

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

// ---------------------------------------------------------------------------
// task progress / mark
// ---------------------------------------------------------------------------

fn run_task_progress(format: OutputFormat, change_dir: PathBuf) -> i32 {
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
            let tasks: Vec<Value> = progress.tasks.iter().map(task_to_json).collect();
            emit_json(json!({
                "total": progress.total,
                "complete": progress.complete,
                "pending": progress.total.saturating_sub(progress.complete),
                "tasks": tasks,
            }));
        }
        OutputFormat::Text => {
            println!("{}/{} tasks complete", progress.complete, progress.total);
            for task in &progress.tasks {
                let mark = if task.complete { "x" } else { " " };
                println!("  [{}] {} {}", mark, task.number, task.description);
            }
        }
    }
    EXIT_SUCCESS
}

fn task_to_json(t: &Task) -> Value {
    let skill = t.skill_directive.as_ref().map(|d| {
        json!({
            "plugin": d.plugin,
            "skill": d.skill,
        })
    });
    json!({
        "group": t.group,
        "number": t.number,
        "description": t.description,
        "complete": t.complete,
        "skill-directive": skill,
    })
}

fn run_task_mark(format: OutputFormat, change_dir: PathBuf, task_number: String) -> i32 {
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
            emit_json(json!({
                "marked": task_number,
                "new-content-path": tasks_path.display().to_string(),
                "idempotent": idempotent,
            }));
        }
        OutputFormat::Text => {
            if idempotent {
                println!("Task {task_number} already complete.");
            } else {
                println!("Marked task {task_number} complete.");
            }
        }
    }
    EXIT_SUCCESS
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

fn resolve_tasks_path_for(
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
// schema resolve / check
// ---------------------------------------------------------------------------

fn run_schema_resolve(format: OutputFormat, schema_value: String, project_dir: PathBuf) -> i32 {
    let resolved = match Schema::resolve(&schema_value, &project_dir) {
        Ok(r) => r,
        Err(err) => return emit_error(format, &err),
    };
    let (source, path) = match &resolved.source {
        SchemaSource::Local(p) => ("local", p.clone()),
        SchemaSource::Cached(p) => ("cached", p.clone()),
    };

    match format {
        OutputFormat::Json => emit_json(json!({
            "schema-value": schema_value,
            "resolved-path": path.display().to_string(),
            "source": source,
        })),
        OutputFormat::Text => println!("{}", path.display()),
    }
    EXIT_SUCCESS
}

fn run_schema_pipeline(format: OutputFormat, phase: Phase, change: Option<PathBuf>) -> i32 {
    let (project_dir, config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let pipeline = match PipelineView::load(&config.schema, &project_dir) {
        Ok(view) => view,
        Err(err) => return emit_error(format, &err),
    };

    let order = match pipeline.topo_order(phase) {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let completion = change.as_deref().map(|change_dir| pipeline.completion_for(phase, change_dir));

    match format {
        OutputFormat::Json => {
            let briefs: Vec<Value> = order
                .iter()
                .map(|b| {
                    let present = completion.as_ref().and_then(|c| c.get(&b.frontmatter.id));
                    json!({
                        "id": b.frontmatter.id,
                        "description": b.frontmatter.description,
                        "path": b.path.display().to_string(),
                        "needs": b.frontmatter.needs,
                        "generates": b.frontmatter.generates,
                        "tracks": b.frontmatter.tracks,
                        "present": present.copied().map(Value::from).unwrap_or(Value::Null),
                    })
                })
                .collect();
            emit_json(json!({
                "phase": phase_label(phase),
                "change": change.as_ref().map(|p| p.display().to_string()),
                "briefs": briefs,
            }));
        }
        OutputFormat::Text => {
            println!("phase: {}", phase_label(phase));
            for b in &order {
                let present_label = completion
                    .as_ref()
                    .and_then(|c| c.get(&b.frontmatter.id))
                    .copied()
                    .map(|p| if p { " [x]" } else { " [ ]" })
                    .unwrap_or("");
                println!("  {}{present_label}", b.frontmatter.id);
                if let Some(g) = &b.frontmatter.generates {
                    println!("    generates: {g}");
                }
                if !b.frontmatter.needs.is_empty() {
                    println!("    needs: {}", b.frontmatter.needs.join(", "));
                }
                if let Some(t) = &b.frontmatter.tracks {
                    println!("    tracks: {t}");
                }
            }
        }
    }
    EXIT_SUCCESS
}

fn run_schema_check(format: OutputFormat, schema_dir: PathBuf) -> i32 {
    let schema_path = schema_dir.join("schema.yaml");
    let text = match std::fs::read_to_string(&schema_path) {
        Ok(t) => t,
        Err(err) => return emit_error(format, &Error::Io(err)),
    };
    let schema: Schema = match serde_yaml::from_str(&text) {
        Ok(s) => s,
        Err(err) => return emit_error(format, &Error::Yaml(err)),
    };
    let results = schema.validate_structure();
    let passed = !results.iter().any(|r| matches!(r, ValidationResult::Fail { .. }));

    match format {
        OutputFormat::Json => {
            let results_json: Vec<Value> = results.iter().map(validation_result_to_json).collect();
            emit_json(json!({
                "passed": passed,
                "results": results_json,
            }));
        }
        OutputFormat::Text => {
            if passed {
                println!("Schema OK");
            } else {
                let fail_count =
                    results.iter().filter(|r| matches!(r, ValidationResult::Fail { .. })).count();
                println!("Schema invalid: {fail_count} errors");
                for r in &results {
                    if let ValidationResult::Fail { rule_id, detail, .. } = r {
                        println!("  [fail] {rule_id}: {detail}");
                    }
                }
            }
        }
    }
    if passed { EXIT_SUCCESS } else { EXIT_VALIDATION_FAILED }
}

fn validation_result_to_json(r: &ValidationResult) -> Value {
    match r {
        ValidationResult::Pass { rule_id, rule } => json!({
            "status": "pass",
            "rule-id": rule_id,
            "rule": rule,
        }),
        ValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => json!({
            "status": "fail",
            "rule-id": rule_id,
            "rule": rule,
            "detail": detail,
        }),
        ValidationResult::Deferred {
            rule_id,
            rule,
            reason,
        } => json!({
            "status": "deferred",
            "rule-id": rule_id,
            "rule": rule,
            "reason": reason,
        }),
    }
}

// ---------------------------------------------------------------------------
// change subcommand tree
// ---------------------------------------------------------------------------

fn run_change(format: OutputFormat, action: ChangeAction) -> i32 {
    match action {
        ChangeAction::Create {
            name,
            schema,
            if_exists,
        } => run_change_create(format, name, schema, if_exists.into()),
        ChangeAction::List => run_status(format, None),
        ChangeAction::Status { name } => run_status(format, Some(name)),
        ChangeAction::Transition { name, target } => run_change_transition(format, name, target),
        ChangeAction::TouchedSpecs { name, scan, set } => {
            run_change_touched_specs(format, name, scan, set)
        }
        ChangeAction::Overlap { name } => run_change_overlap(format, name),
        ChangeAction::Archive { name } => run_change_archive(format, name),
        ChangeAction::Drop { name, reason } => run_change_drop(format, name, reason),
        ChangeAction::PhaseOutcome {
            name,
            phase,
            outcome,
            summary,
            context,
        } => run_change_phase_outcome(format, name, phase, outcome, summary, context),
        ChangeAction::Outcome { name } => run_change_outcome(format, name),
        ChangeAction::JournalAppend {
            name,
            phase,
            kind,
            summary,
            context,
        } => run_change_journal_append(format, name, phase, kind, summary, context),
    }
}

fn run_change_create(
    format: OutputFormat, name: String, schema: Option<String>, if_exists: CreateIfExists,
) -> i32 {
    let (project_dir, config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let schema_value = schema.unwrap_or_else(|| config.schema.clone());
    let changes_dir = ProjectConfig::changes_dir(&project_dir);
    if let Err(err) = std::fs::create_dir_all(&changes_dir) {
        return emit_error(format, &Error::Io(err));
    }

    let outcome =
        match change_actions::create(&changes_dir, &name, &schema_value, if_exists, Utc::now()) {
            Ok(outcome) => outcome,
            Err(err) => return emit_error(format, &err),
        };

    emit_change_create(format, &outcome)
}

fn emit_change_create(format: OutputFormat, outcome: &CreateOutcome) -> i32 {
    match format {
        OutputFormat::Json => emit_json(json!({
            "name": outcome.change_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            "change-dir": outcome.change_dir.display().to_string(),
            "status": format!("{:?}", outcome.metadata.status).to_lowercase(),
            "schema": outcome.metadata.schema,
            "created": outcome.created,
            "restarted": outcome.restarted,
        })),
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
            println!("  status: {}", format!("{:?}", outcome.metadata.status).to_lowercase());
        }
    }
    EXIT_SUCCESS
}

fn run_change_transition(format: OutputFormat, name: String, target: LifecycleStatus) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    let metadata = match change_actions::transition(&change_dir, target, Utc::now()) {
        Ok(meta) => meta,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => emit_json(json!({
            "name": name,
            "status": format!("{:?}", metadata.status).to_lowercase(),
            "defined-at": metadata.defined_at,
            "build-started-at": metadata.build_started_at,
            "completed-at": metadata.completed_at,
            "merged-at": metadata.merged_at,
            "dropped-at": metadata.dropped_at,
        })),
        OutputFormat::Text => {
            println!("{name}: status = {}", format!("{:?}", metadata.status).to_lowercase());
        }
    }
    EXIT_SUCCESS
}

fn run_change_touched_specs(
    format: OutputFormat, name: String, scan: bool, set: Vec<String>,
) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    let specs_dir = ProjectConfig::specs_dir(&project_dir);

    let entries = if !set.is_empty() {
        match parse_touched_spec_set(&set) {
            Ok(v) => {
                let metadata = match change_actions::write_touched_specs(&change_dir, v.clone()) {
                    Ok(m) => m,
                    Err(err) => return emit_error(format, &err),
                };
                metadata.touched_specs
            }
            Err(err) => return emit_error(format, &err),
        }
    } else if scan {
        let scanned = match change_actions::scan_touched_specs(&change_dir, &specs_dir) {
            Ok(v) => v,
            Err(err) => return emit_error(format, &err),
        };
        let metadata = match change_actions::write_touched_specs(&change_dir, scanned) {
            Ok(m) => m,
            Err(err) => return emit_error(format, &err),
        };
        metadata.touched_specs
    } else {
        // Read-only: report the current touched_specs without mutating.
        let metadata = match ChangeMetadata::load(&change_dir) {
            Ok(m) => m,
            Err(err) => return emit_error(format, &err),
        };
        metadata.touched_specs
    };

    match format {
        OutputFormat::Json => emit_json(json!({
            "name": name,
            "touched-specs": touched_specs_to_json(&entries),
        })),
        OutputFormat::Text => {
            if entries.is_empty() {
                println!("{name}: no touched specs");
            } else {
                println!("{name}:");
                for entry in &entries {
                    println!("  {} ({})", entry.name, spec_type_label(entry.spec_type));
                }
            }
        }
    }
    EXIT_SUCCESS
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

fn run_change_overlap(format: OutputFormat, name: String) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let changes_dir = ProjectConfig::changes_dir(&project_dir);
    let overlaps = match change_actions::overlap(&changes_dir, &name) {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => emit_json(json!({
            "name": name,
            "overlaps": overlaps.iter().map(overlap_to_json).collect::<Vec<_>>(),
        })),
        OutputFormat::Text => {
            if overlaps.is_empty() {
                println!("{name}: no overlapping changes");
            } else {
                for o in &overlaps {
                    println!(
                        "{}: also touched by `{}` ({} vs {})",
                        o.capability,
                        o.other_change,
                        spec_type_label(o.our_spec_type),
                        spec_type_label(o.other_spec_type),
                    );
                }
            }
        }
    }
    EXIT_SUCCESS
}

fn run_change_archive(format: OutputFormat, name: String) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    let archive_dir = ProjectConfig::archive_dir(&project_dir);
    let target = match change_actions::archive(&change_dir, &archive_dir, Utc::now()) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => emit_json(json!({
            "name": name,
            "archive-path": target.display().to_string(),
        })),
        OutputFormat::Text => {
            println!("{name}: archived to {}", target.display());
        }
    }
    EXIT_SUCCESS
}

fn run_change_drop(format: OutputFormat, name: String, reason: Option<String>) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    let archive_dir = ProjectConfig::archive_dir(&project_dir);
    let (metadata, archive_path) =
        match change_actions::drop(&change_dir, &archive_dir, reason.as_deref(), Utc::now()) {
            Ok(pair) => pair,
            Err(err) => return emit_error(format, &err),
        };

    match format {
        OutputFormat::Json => emit_json(json!({
            "name": name,
            "status": format!("{:?}", metadata.status).to_lowercase(),
            "archive-path": archive_path.display().to_string(),
            "drop-reason": metadata.drop_reason,
        })),
        OutputFormat::Text => {
            println!("{name}: dropped and archived to {}", archive_path.display());
            if let Some(r) = &metadata.drop_reason {
                println!("  reason: {r}");
            }
        }
    }
    EXIT_SUCCESS
}

fn run_change_phase_outcome(
    format: OutputFormat, name: String, phase: Phase, outcome: Outcome, summary: String,
    context: Option<String>,
) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    if !change_dir.is_dir() || !ChangeMetadata::path(&change_dir).exists() {
        let err = Error::Config(format!("change '{name}' not found at {}", change_dir.display()));
        return emit_error(format, &err);
    }

    let metadata = match change_actions::phase_outcome(
        &change_dir,
        phase,
        outcome,
        &summary,
        context.as_deref(),
        Utc::now(),
    ) {
        Ok(m) => m,
        Err(err) => return emit_error(format, &err),
    };

    let stamped = metadata
        .outcome
        .as_ref()
        .expect("phase_outcome action must set metadata.outcome on success");
    let phase_str = phase_label(phase);
    let outcome_str = outcome_label(outcome);

    match format {
        OutputFormat::Json => emit_json(json!({
            "change": name,
            "phase": phase_str,
            "outcome": outcome_str,
            "at": stamped.at,
        })),
        OutputFormat::Text => {
            println!("Stamped outcome '{outcome_str}' for phase '{phase_str}' on change '{name}'.");
        }
    }
    EXIT_SUCCESS
}

/// Report the stamped `.metadata.yaml.outcome` for `name`.
///
/// Symmetric with [`run_change_phase_outcome`] (the writer): this is
/// the read verb `/spec:execute` consumes after a phase returns.
/// Emits `"outcome": null` when the change exists but nothing has
/// been stamped; exits `EXIT_SUCCESS` in both cases — an unstamped
/// change is not an error, just an absence.
fn run_change_outcome(format: OutputFormat, name: String) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    if !change_dir.is_dir() || !ChangeMetadata::path(&change_dir).exists() {
        let err = Error::Config(format!("change '{name}' not found at {}", change_dir.display()));
        return emit_error(format, &err);
    }

    let metadata = match ChangeMetadata::load(&change_dir) {
        Ok(m) => m,
        Err(err) => return emit_error(format, &err),
    };

    match format {
        OutputFormat::Json => {
            // Build the outcome payload explicitly so `context` is
            // emitted as `null` when absent (the canonical shape
            // `/spec:execute` pattern-matches on). `PhaseOutcome`'s
            // serde derive skips `None` contexts on disk; the CLI
            // contract is the stable null.
            let outcome_json = match &metadata.outcome {
                Some(o) => json!({
                    "phase": phase_label(o.phase),
                    "outcome": outcome_label(o.outcome),
                    "at": o.at,
                    "summary": o.summary,
                    "context": o.context.clone().map(Value::from).unwrap_or(Value::Null),
                }),
                None => Value::Null,
            };
            emit_json(json!({
                "name": name,
                "outcome": outcome_json,
            }));
        }
        OutputFormat::Text => match &metadata.outcome {
            Some(o) => {
                let phase = phase_label(o.phase);
                let outcome = outcome_label(o.outcome);
                println!("{name}: {phase}/{outcome} — {}", o.summary);
            }
            None => {
                println!("{name}: no outcome stamped");
            }
        },
    }
    EXIT_SUCCESS
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Plan => "plan",
        Phase::Define => "define",
        Phase::Build => "build",
        Phase::Merge => "merge",
    }
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Success => "success",
        Outcome::Failure => "failure",
        Outcome::Deferred => "deferred",
    }
}

fn run_change_journal_append(
    format: OutputFormat, name: String, phase: Phase, kind: EntryKind, summary: String,
    context: Option<String>,
) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let change_dir = ProjectConfig::changes_dir(&project_dir).join(&name);
    if !change_dir.is_dir() || !ChangeMetadata::path(&change_dir).exists() {
        let err = Error::Config(format!("change '{name}' not found at {}", change_dir.display()));
        return emit_error(format, &err);
    }

    let timestamp = format_rfc3339(Utc::now());
    let entry = JournalEntry {
        timestamp: timestamp.clone(),
        step: phase,
        r#type: kind,
        summary: summary.clone(),
        context: context.clone(),
    };

    if let Err(err) = Journal::append(&change_dir, entry) {
        return emit_error(format, &err);
    }

    let phase_str = phase_label(phase);
    let kind_str = entry_kind_label(kind);

    match format {
        OutputFormat::Json => emit_json(json!({
            "change": name,
            "phase": phase_str,
            "kind": kind_str,
            "timestamp": timestamp,
        })),
        OutputFormat::Text => {
            println!("Appended {kind_str} entry to {name}/journal.yaml.");
        }
    }
    EXIT_SUCCESS
}

fn entry_kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Question => "question",
        EntryKind::Failure => "failure",
        EntryKind::Recovery => "recovery",
    }
}

fn overlap_to_json(o: &Overlap) -> Value {
    json!({
        "capability": o.capability,
        "other-change": o.other_change,
        "our-spec-type": spec_type_label(o.our_spec_type),
        "other-spec-type": spec_type_label(o.other_spec_type),
    })
}

fn touched_specs_to_json(entries: &[TouchedSpec]) -> Vec<Value> {
    entries
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "type": spec_type_label(t.spec_type),
            })
        })
        .collect()
}

fn spec_type_label(t: SpecType) -> &'static str {
    match t {
        SpecType::New => "new",
        SpecType::Modified => "modified",
    }
}

// ---------------------------------------------------------------------------
// initiative subcommand tree (read-only: validate, next, status)
// ---------------------------------------------------------------------------

fn run_initiative(format: OutputFormat, action: InitiativeAction) -> i32 {
    match action {
        InitiativeAction::Init { name, sources } => run_initiative_init(format, name, sources),
        InitiativeAction::Validate => run_initiative_validate(format),
        InitiativeAction::Next => run_initiative_next(format),
        InitiativeAction::Status => run_initiative_status(format),
        InitiativeAction::Create {
            name,
            depends_on,
            affects,
            sources,
            description,
        } => run_initiative_create(format, name, depends_on, affects, sources, description),
        InitiativeAction::Amend {
            name,
            depends_on,
            affects,
            sources,
            description,
        } => run_initiative_amend(format, name, depends_on, affects, sources, description),
        InitiativeAction::Transition { name, target, reason } => {
            run_initiative_transition(format, name, target, reason)
        }
        InitiativeAction::Archive { force } => run_initiative_archive(format, force),
        InitiativeAction::Lock { action } => match action {
            LockAction::Acquire { pid } => run_initiative_lock_acquire(format, pid),
            LockAction::Release { pid } => run_initiative_lock_release(format, pid),
            LockAction::Status => run_initiative_lock_status(format),
        },
    }
}

/// `<project_dir>/.specify/plan.yaml`.
fn plan_file_path(project_dir: &Path) -> PathBuf {
    ProjectConfig::specify_dir(project_dir).join("plan.yaml")
}

/// Ensure the plan file exists before we try to load it. Error text is
/// the stable "plan file not found: .specify/plan.yaml" string that
/// skill authors match on.
fn require_plan_file(project_dir: &Path) -> Result<PathBuf, Error> {
    let path = plan_file_path(project_dir);
    if !path.exists() {
        return Err(Error::Config("plan file not found: .specify/plan.yaml".to_string()));
    }
    Ok(path)
}

fn plan_status_label(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Pending => "pending",
        PlanStatus::InProgress => "in-progress",
        PlanStatus::Done => "done",
        PlanStatus::Blocked => "blocked",
        PlanStatus::Failed => "failed",
        PlanStatus::Skipped => "skipped",
    }
}

fn plan_validation_level_label(level: &PlanValidationLevel) -> &'static str {
    match level {
        PlanValidationLevel::Error => "error",
        PlanValidationLevel::Warning => "warning",
    }
}

/// Parse a single `--source <key>=<path-or-url>` CLI value into a
/// `(key, value)` pair. Returns a `String` error on malformed input so
/// clap surfaces a standard usage diagnostic (exit code 2).
fn parse_source_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("--source must be <key>=<path-or-url>, got `{s}`"))?;
    if k.is_empty() || v.is_empty() {
        return Err(format!("--source key and value must be non-empty, got `{s}`"));
    }
    Ok((k.to_string(), v.to_string()))
}

fn run_initiative_init(format: OutputFormat, name: String, sources: Vec<(String, String)>) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    let plan_path = plan_file_path(&project_dir);
    if plan_path.exists() {
        let err = Error::Config(format!(
            "plan already exists at {}; run `specify initiative archive` first",
            plan_path.display()
        ));
        return emit_error(format, &err);
    }

    // Fold the CLI vector into a BTreeMap, rejecting duplicate keys
    // before they silently clobber earlier values.
    let mut source_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (k, v) in sources {
        if source_map.contains_key(&k) {
            let err = Error::Config(format!("duplicate key `{k}` in --source arguments"));
            return emit_error(format, &err);
        }
        source_map.insert(k, v);
    }

    let plan = match Plan::init(&name, source_map) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    if let Err(err) = plan.save(&plan_path) {
        return emit_error(format, &err);
    }

    match format {
        OutputFormat::Json => emit_json(json!({
            "plan": {
                "name": name,
                "path": absolute_string(&plan_path),
            },
        })),
        OutputFormat::Text => {
            println!("Initialised plan '{name}' at {}.", plan_path.display());
        }
    }
    EXIT_SUCCESS
}

fn run_initiative_validate(format: OutputFormat) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let plan_path = match require_plan_file(&project_dir) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let plan = match Plan::load(&plan_path) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let changes_dir = ProjectConfig::changes_dir(&project_dir);

    let results = plan.validate(Some(&changes_dir));
    let has_errors = results.iter().any(|r| matches!(r.level, PlanValidationLevel::Error));

    match format {
        OutputFormat::Json => {
            let items: Vec<Value> = results.iter().map(plan_validation_to_json).collect();
            emit_json(json!({
                "plan": {
                    "name": plan.name,
                    "path": plan_path.display().to_string(),
                },
                "results": items,
                "passed": !has_errors,
            }));
        }
        OutputFormat::Text => {
            for r in &results {
                print_plan_validation_line(r);
            }
            if results.is_empty() {
                println!("Plan OK");
            }
        }
    }

    if has_errors { EXIT_VALIDATION_FAILED } else { EXIT_SUCCESS }
}

fn plan_validation_to_json(r: &PlanValidationResult) -> Value {
    json!({
        "level": plan_validation_level_label(&r.level),
        "code": r.code,
        "entry": r.entry,
        "message": r.message,
    })
}

/// Roughly-columnar single line per finding. Not golden-tested — skills
/// that need structure consume `--format json`.
fn print_plan_validation_line(r: &PlanValidationResult) {
    let level = match r.level {
        PlanValidationLevel::Error => "ERROR  ",
        PlanValidationLevel::Warning => "WARNING",
    };
    let entry_col = match &r.entry {
        Some(e) => format!("[{e}]"),
        None => String::new(),
    };
    println!("{level} {:<32} {:<24} {}", r.code, entry_col, r.message);
}

/// Emit the stable "go run `specify initiative validate`" pointer when
/// `initiative next` or `initiative status` is asked to operate on a
/// structurally broken plan.
fn emit_plan_structural_error(format: OutputFormat) -> i32 {
    let msg = "plan has structural errors; run 'specify initiative validate' for detail";
    match format {
        OutputFormat::Json => emit_json(json!({
            "error": "validation",
            "message": msg,
            "exit-code": EXIT_VALIDATION_FAILED,
        })),
        OutputFormat::Text => eprintln!("error: {msg}"),
    }
    EXIT_VALIDATION_FAILED
}

fn run_initiative_next(format: OutputFormat) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let plan_path = match require_plan_file(&project_dir) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let plan = match Plan::load(&plan_path) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let changes_dir = ProjectConfig::changes_dir(&project_dir);

    let results = plan.validate(Some(&changes_dir));
    if results.iter().any(|r| matches!(r.level, PlanValidationLevel::Error)) {
        return emit_plan_structural_error(format);
    }

    if let Some(active) = plan.changes.iter().find(|c| c.status == PlanStatus::InProgress) {
        match format {
            OutputFormat::Json => emit_json(json!({
                "next": Value::Null,
                "reason": "in-progress",
                "active": active.name,
            })),
            OutputFormat::Text => println!("Active change in progress: {}", active.name),
        }
        return EXIT_SUCCESS;
    }

    match plan.next_eligible() {
        Some(entry) => match format {
            OutputFormat::Json => emit_json(json!({
                "next": entry.name,
                "reason": Value::Null,
                "active": Value::Null,
            })),
            OutputFormat::Text => println!("{}", entry.name),
        },
        None => {
            // Classify the "None" branch: fully-finished initiative vs
            // still-has-work-but-blocked. An empty plan falls out of the
            // `all` check as "all-done" (vacuously true).
            let all_terminal = plan
                .changes
                .iter()
                .all(|c| matches!(c.status, PlanStatus::Done | PlanStatus::Skipped));
            let (reason, text_msg) = if all_terminal {
                ("all-done", "All changes done.")
            } else {
                (
                    "stuck",
                    "No eligible changes — remaining entries are blocked, failed, or waiting on unmet dependencies.",
                )
            };
            match format {
                OutputFormat::Json => emit_json(json!({
                    "next": Value::Null,
                    "reason": reason,
                    "active": Value::Null,
                })),
                OutputFormat::Text => println!("{text_msg}"),
            }
        }
    }
    EXIT_SUCCESS
}

fn run_initiative_status(format: OutputFormat) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let plan_path = match require_plan_file(&project_dir) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let plan = match Plan::load(&plan_path) {
        Ok(p) => p,
        Err(err) => return emit_error(format, &err),
    };
    let changes_dir = ProjectConfig::changes_dir(&project_dir);

    let results = plan.validate(Some(&changes_dir));
    // Cycle is recoverable (we fall back to list order); any *other*
    // structural error (duplicate-name / unknown-depends-on / unknown-
    // affects / unknown-source / multiple-in-progress) is fatal.
    let has_other_structural_errors = results
        .iter()
        .any(|r| matches!(r.level, PlanValidationLevel::Error) && r.code != "dependency-cycle");
    if has_other_structural_errors {
        return emit_plan_structural_error(format);
    }

    let (ordered, order_label) = match plan.topological_order() {
        Ok(v) => (v, "topological"),
        Err(_) => {
            match format {
                OutputFormat::Json => {
                    eprintln!(
                        "warning: dependency cycle detected — falling back to list order. Run 'specify initiative validate' for detail."
                    );
                }
                OutputFormat::Text => {
                    println!(
                        "⚠ dependency cycle detected — falling back to list order. Run 'specify initiative validate' for detail."
                    );
                }
            }
            (plan.changes.iter().collect::<Vec<_>>(), "list")
        }
    };

    let mut counts: BTreeMap<PlanStatus, usize> = PlanStatus::ALL.iter().map(|&s| (s, 0)).collect();
    for entry in &plan.changes {
        *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
    }
    let total: usize = counts.values().sum();

    let active = plan.changes.iter().find(|c| c.status == PlanStatus::InProgress);
    let active_lifecycle =
        active.map(|a| load_lifecycle_label(&changes_dir.join(&a.name))).unwrap_or(None);

    let blocked: Vec<&PlanChange> =
        plan.changes.iter().filter(|c| c.status == PlanStatus::Blocked).collect();
    let failed: Vec<&PlanChange> =
        plan.changes.iter().filter(|c| c.status == PlanStatus::Failed).collect();

    let next_eligible = plan.next_eligible();

    let impact = compute_impact(&plan);

    match format {
        OutputFormat::Json => {
            let entries: Vec<Value> = ordered
                .iter()
                .map(|entry| {
                    let lifecycle = if entry.status == PlanStatus::InProgress {
                        active_lifecycle.clone()
                    } else {
                        None
                    };
                    plan_entry_to_json(entry, lifecycle)
                })
                .collect();

            let blocked_json: Vec<Value> = blocked
                .iter()
                .map(|c| json!({"name": c.name, "reason": c.status_reason}))
                .collect();
            let failed_json: Vec<Value> =
                failed.iter().map(|c| json!({"name": c.name, "reason": c.status_reason})).collect();

            let active_json = active.map(|a| {
                json!({
                    "name": a.name,
                    "lifecycle": active_lifecycle,
                })
            });

            let impact_json: Vec<Value> = impact
                .iter()
                .map(|(done_name, referenced_by)| {
                    json!({
                        "done": done_name,
                        "referenced-by": referenced_by,
                    })
                })
                .collect();

            emit_json(json!({
                "plan": {
                    "name": plan.name,
                    "path": plan_path.display().to_string(),
                },
                "counts": {
                    "done": counts[&PlanStatus::Done],
                    "in-progress": counts[&PlanStatus::InProgress],
                    "pending": counts[&PlanStatus::Pending],
                    "blocked": counts[&PlanStatus::Blocked],
                    "failed": counts[&PlanStatus::Failed],
                    "skipped": counts[&PlanStatus::Skipped],
                    "total": total,
                },
                "order": order_label,
                "entries": entries,
                "in-progress": active_json,
                "blocked": blocked_json,
                "failed": failed_json,
                "next-eligible": next_eligible.map(|e| e.name.clone()),
                "impact": impact_json,
            }));
        }
        OutputFormat::Text => print_plan_status_text(&PlanStatusView {
            plan: &plan,
            counts: &counts,
            active,
            active_lifecycle: active_lifecycle.as_deref(),
            blocked: &blocked,
            failed: &failed,
            next_eligible,
            impact: &impact,
        }),
    }
    EXIT_SUCCESS
}

/// All the slices `print_plan_status_text` needs. Bundled so the
/// function takes one `&PlanStatusView` instead of eight positional
/// arguments.
struct PlanStatusView<'a> {
    plan: &'a Plan,
    counts: &'a BTreeMap<PlanStatus, usize>,
    active: Option<&'a PlanChange>,
    active_lifecycle: Option<&'a str>,
    blocked: &'a [&'a PlanChange],
    failed: &'a [&'a PlanChange],
    next_eligible: Option<&'a PlanChange>,
    impact: &'a [(String, Vec<String>)],
}

/// Best-effort load of `<change_dir>/.metadata.yaml` to surface the
/// lifecycle state of the in-progress change. Missing metadata returns
/// `None` — status rendering treats it as "no change dir yet".
fn load_lifecycle_label(change_dir: &Path) -> Option<String> {
    if !ChangeMetadata::path(change_dir).exists() {
        return None;
    }
    ChangeMetadata::load(change_dir).ok().map(|m| format!("{:?}", m.status).to_lowercase())
}

fn plan_entry_to_json(entry: &PlanChange, lifecycle: Option<String>) -> Value {
    json!({
        "name": entry.name,
        "status": plan_status_label(entry.status),
        "depends-on": entry.depends_on,
        "affects": entry.affects,
        "sources": entry.sources,
        "status-reason": entry.status_reason,
        "description": entry.description,
        "lifecycle": lifecycle,
    })
}

/// For every `Done` entry, list pending/in-progress/blocked entries
/// whose `affects` references it. Pairs are emitted in plan list order
/// (both the outer `done` and the inner `referenced-by` list) so the
/// report is deterministic regardless of `HashMap` iteration order.
fn compute_impact(plan: &Plan) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for done in plan.changes.iter().filter(|c| c.status == PlanStatus::Done) {
        let refs: Vec<String> = plan
            .changes
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    PlanStatus::Pending | PlanStatus::InProgress | PlanStatus::Blocked
                )
            })
            .filter(|c| c.affects.iter().any(|a| a == &done.name))
            .map(|c| c.name.clone())
            .collect();
        if !refs.is_empty() {
            out.push((done.name.clone(), refs));
        }
    }
    out
}

fn print_plan_status_text(view: &PlanStatusView) {
    let counts = view.counts;
    let total: usize = counts.values().sum();
    println!("## Initiative: {}", view.plan.name);
    println!();
    println!();
    println!(
        "Progress: done {}, in-progress {}, pending {}, blocked {}, failed {}, skipped {} (total {total})",
        counts[&PlanStatus::Done],
        counts[&PlanStatus::InProgress],
        counts[&PlanStatus::Pending],
        counts[&PlanStatus::Blocked],
        counts[&PlanStatus::Failed],
        counts[&PlanStatus::Skipped],
    );

    if let Some(a) = view.active {
        let lifecycle_label = view.active_lifecycle.unwrap_or("<no change dir yet>");
        println!();
        println!("In progress: {} (lifecycle: {lifecycle_label})", a.name);
    }

    if !view.blocked.is_empty() {
        println!();
        println!("Blocked:");
        for c in view.blocked {
            let reason = c.status_reason.as_deref().unwrap_or("-");
            println!("  - {} (reason: {reason})", c.name);
        }
    }

    if !view.failed.is_empty() {
        println!();
        println!("Failed:");
        for c in view.failed {
            let reason = c.status_reason.as_deref().unwrap_or("-");
            println!("  - {} (reason: {reason})", c.name);
        }
    }

    println!();
    match view.next_eligible {
        Some(e) => println!("Next eligible: {}", e.name),
        None => println!("Next eligible: — (waiting on dependencies / all done)"),
    }

    if !view.impact.is_empty() {
        println!();
        for (done, refs) in view.impact {
            println!("Impact: {done} is referenced by pending changes: [{}]", refs.join(", "));
        }
    }
}

// ---------------------------------------------------------------------------
// initiative subcommand tree (write-side: create, amend, transition)
// ---------------------------------------------------------------------------

fn load_plan_for_write(format: OutputFormat) -> Result<(PathBuf, PathBuf, Plan), i32> {
    let (project_dir, _config) = require_project().map_err(|err| emit_error(format, &err))?;
    let plan_path = require_plan_file(&project_dir).map_err(|err| emit_error(format, &err))?;
    let plan = Plan::load(&plan_path).map_err(|err| emit_error(format, &err))?;
    Ok((project_dir, plan_path, plan))
}

fn plan_ref_json(plan: &Plan, plan_path: &Path) -> Value {
    json!({
        "name": plan.name,
        "path": plan_path.display().to_string(),
    })
}

/// Serialize a `PlanChange` into the on-the-wire kebab-case JSON shape
/// (matches the fields emitted by `plan status.entries[]`, minus the
/// `lifecycle` overlay which is a status-report concern).
fn plan_change_entry_json(entry: &PlanChange) -> Value {
    serde_json::to_value(entry).expect("PlanChange serialises as JSON")
}

fn run_initiative_create(
    format: OutputFormat, name: String, depends_on: Vec<String>, affects: Vec<String>,
    sources: Vec<String>, description: Option<String>,
) -> i32 {
    let (_project_dir, plan_path, mut plan) = match load_plan_for_write(format) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let entry = PlanChange {
        name: name.clone(),
        status: PlanStatus::Pending,
        depends_on,
        affects,
        sources,
        description,
        status_reason: None,
    };

    if let Err(err) = plan.create(entry) {
        return emit_error(format, &err);
    }
    if let Err(err) = plan.save(&plan_path) {
        return emit_error(format, &err);
    }

    // `Plan::create` forces status to Pending and clears status_reason, so
    // the freshly-appended entry is always the tail of `plan.changes`.
    let created = plan.changes.last().expect("Plan::create appended an entry that is now missing");

    match format {
        OutputFormat::Json => emit_json(json!({
            "plan": plan_ref_json(&plan, &plan_path),
            "action": "create",
            "entry": plan_change_entry_json(created),
        })),
        OutputFormat::Text => {
            println!("Created plan entry '{name}' with status 'pending'.");
        }
    }
    EXIT_SUCCESS
}

fn run_initiative_amend(
    format: OutputFormat, name: String, depends_on: Option<Vec<String>>,
    affects: Option<Vec<String>>, sources: Option<Vec<String>>, description: Option<String>,
) -> i32 {
    let (_project_dir, plan_path, mut plan) = match load_plan_for_write(format) {
        Ok(v) => v,
        Err(code) => return code,
    };

    // Map clap's `Option<String>` for description into the library's
    // three-way `Option<Option<String>>`: absent = None, "" = clear,
    // otherwise replace.
    let description_patch: Option<Option<String>> =
        description.map(|s| if s.is_empty() { None } else { Some(s) });

    let patch = PlanChangePatch {
        depends_on,
        affects,
        sources,
        description: description_patch,
    };

    if let Err(err) = plan.amend(&name, patch) {
        return emit_error(format, &err);
    }
    if let Err(err) = plan.save(&plan_path) {
        return emit_error(format, &err);
    }

    let amended = plan.changes.iter().find(|c| c.name == name).expect("amended entry present");

    match format {
        OutputFormat::Json => emit_json(json!({
            "plan": plan_ref_json(&plan, &plan_path),
            "action": "amend",
            "entry": plan_change_entry_json(amended),
        })),
        OutputFormat::Text => {
            println!("Amended plan entry '{name}'.");
        }
    }
    EXIT_SUCCESS
}

fn run_initiative_transition(
    format: OutputFormat, name: String, target: PlanStatus, reason: Option<String>,
) -> i32 {
    let (_project_dir, plan_path, mut plan) = match load_plan_for_write(format) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let old_status = match plan.changes.iter().find(|c| c.name == name) {
        Some(c) => c.status,
        None => {
            return emit_error(format, &Error::Config(format!("no change named '{name}' in plan")));
        }
    };

    if let Err(err) = plan.transition(&name, target, reason.as_deref()) {
        return emit_error(format, &err);
    }
    if let Err(err) = plan.save(&plan_path) {
        return emit_error(format, &err);
    }

    let entry = plan.changes.iter().find(|c| c.name == name).expect("transitioned entry present");

    match format {
        OutputFormat::Json => emit_json(json!({
            "plan": plan_ref_json(&plan, &plan_path),
            "entry": {
                "name": entry.name,
                "status": plan_status_label(entry.status),
                "status-reason": entry.status_reason,
            },
        })),
        OutputFormat::Text => {
            println!(
                "Transitioned '{name}': {} → {}.",
                plan_status_label(old_status),
                plan_status_label(entry.status),
            );
        }
    }
    EXIT_SUCCESS
}

// ---------------------------------------------------------------------------
// plan archive
// ---------------------------------------------------------------------------

fn run_initiative_archive(format: OutputFormat, force: bool) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let plan_path = project_dir.join(".specify/plan.yaml");
    if !plan_path.exists() {
        let err = Error::Config("plan file not found: .specify/plan.yaml".to_string());
        return emit_error(format, &err);
    }
    let archive_dir = ProjectConfig::archive_dir(&project_dir).join("plans");

    // Grab the plan name up-front so we can surface it in the
    // success payload even though `Plan::archive` only returns the
    // archived path.
    let plan_name = match Plan::load(&plan_path) {
        Ok(p) => p.name,
        Err(err) => return emit_error(format, &err),
    };

    match Plan::archive(&plan_path, &archive_dir, force) {
        Ok((archived, archived_plans_dir)) => match format {
            OutputFormat::Json => {
                emit_json(json!({
                    "archived": absolute_string(&archived),
                    "archived-plans-dir": archived_plans_dir
                        .as_deref()
                        .map(absolute_string),
                    "plan": { "name": plan_name },
                }));
                EXIT_SUCCESS
            }
            OutputFormat::Text => {
                match archived_plans_dir {
                    Some(dir) => println!(
                        "Archived plan to {}. Working directory moved to {}.",
                        archived.display(),
                        dir.display()
                    ),
                    None => println!("Archived plan to {}.", archived.display()),
                }
                EXIT_SUCCESS
            }
        },
        Err(Error::PlanHasOutstandingWork { entries }) => {
            match format {
                OutputFormat::Json => {
                    emit_json(json!({
                        "error": "plan-has-outstanding-work",
                        "entries": entries,
                        "exit-code": EXIT_GENERIC_FAILURE,
                    }));
                }
                OutputFormat::Text => {
                    eprintln!(
                        "Refusing to archive — outstanding non-terminal entries: {}. Re-run with --force to archive anyway.",
                        entries.join(", ")
                    );
                }
            }
            EXIT_GENERIC_FAILURE
        }
        Err(err) => emit_error(format, &err),
    }
}

// ---------------------------------------------------------------------------
// plan lock {acquire, release, status}
// ---------------------------------------------------------------------------

fn run_initiative_lock_acquire(format: OutputFormat, pid: Option<u32>) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let our_pid = pid.unwrap_or_else(std::process::id);

    match PlanLockStamp::acquire(&project_dir, our_pid) {
        Ok(acquired) => emit_plan_lock_acquired(format, &acquired),
        Err(err) => emit_error(format, &err),
    }
}

fn emit_plan_lock_acquired(format: OutputFormat, acquired: &PlanLockAcquired) -> i32 {
    match format {
        OutputFormat::Json => emit_json(json!({
            "held": true,
            "pid": acquired.pid,
            "already-held": acquired.already_held,
            "reclaimed-stale-pid": acquired.reclaimed_stale_pid,
        })),
        OutputFormat::Text => {
            if acquired.already_held {
                println!("Lock already held by pid {}; re-stamped.", acquired.pid);
            } else {
                println!("Acquired plan lock for pid {}.", acquired.pid);
            }
            if let Some(stale) = acquired.reclaimed_stale_pid {
                println!("  (reclaimed stale stamp from pid {stale})");
            }
        }
    }
    EXIT_SUCCESS
}

fn run_initiative_lock_release(format: OutputFormat, pid: Option<u32>) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let our_pid = pid.unwrap_or_else(std::process::id);

    match PlanLockStamp::release(&project_dir, our_pid) {
        Ok(outcome) => emit_plan_lock_released(format, our_pid, &outcome),
        Err(err) => emit_error(format, &err),
    }
}

/// Mirrors the four [`PlanLockReleased`] outcomes onto the CLI
/// response. All four exit 0 — a mismatched holder is a warning, not
/// an error, per RFC-2 §"Driver Concurrency" (stale reclaim is the
/// self-heal path's job, not release's).
fn emit_plan_lock_released(format: OutputFormat, our_pid: u32, outcome: &PlanLockReleased) -> i32 {
    match format {
        OutputFormat::Json => {
            let payload = match outcome {
                PlanLockReleased::Removed { pid } => json!({
                    "result": "removed",
                    "pid": pid,
                }),
                PlanLockReleased::WasAbsent => json!({
                    "result": "was-absent",
                    "pid": Value::Null,
                }),
                PlanLockReleased::HeldByOther { pid } => json!({
                    "result": "held-by-other",
                    "pid": pid,
                    "our-pid": our_pid,
                }),
            };
            emit_json(payload);
        }
        OutputFormat::Text => match outcome {
            PlanLockReleased::Removed { pid } => {
                println!("Released plan lock held by pid {pid}.");
            }
            PlanLockReleased::WasAbsent => {
                println!("No plan lock to release.");
            }
            PlanLockReleased::HeldByOther { pid: Some(other) } => {
                eprintln!(
                    "warning: plan lock is held by pid {other}, not {our_pid}; not removing."
                );
            }
            PlanLockReleased::HeldByOther { pid: None } => {
                eprintln!(
                    "warning: plan lock contents are malformed; refusing to clobber (run the L2.G self-heal path)."
                );
            }
        },
    }
    EXIT_SUCCESS
}

fn run_initiative_lock_status(format: OutputFormat) -> i32 {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    match PlanLockStamp::status(&project_dir) {
        Ok(state) => emit_plan_lock_state(format, &state),
        Err(err) => emit_error(format, &err),
    }
}

fn emit_plan_lock_state(format: OutputFormat, state: &PlanLockState) -> i32 {
    match format {
        OutputFormat::Json => emit_json(json!({
            "held": state.held,
            "pid": state.pid,
            "stale": state.stale,
        })),
        OutputFormat::Text => match state.pid {
            Some(pid) => {
                let stale = state.stale.unwrap_or(false);
                if stale {
                    println!("stale (pid {pid} no longer alive)");
                } else {
                    println!("held by pid {pid}");
                }
            }
            None => match state.stale {
                Some(true) => println!("stale (malformed lockfile contents)"),
                _ => println!("no lock"),
            },
        },
    }
    EXIT_SUCCESS
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn current_dir() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(Error::Io)
}

/// Load `.specify/project.yaml` from the current directory, running
/// the CLI version-floor check in the process. Every subcommand that
/// touches `.specify/` routes through this so the error shape for
/// "not initialised" / "CLI too old" is uniform.
fn require_project() -> Result<(PathBuf, ProjectConfig), Error> {
    let project_dir = current_dir()?;
    let config = ProjectConfig::load(&project_dir)?;
    Ok((project_dir, config))
}

fn emit_error(format: OutputFormat, err: &Error) -> i32 {
    let code = exit_code_for(err);
    match format {
        OutputFormat::Json => emit_json_error(err, code),
        OutputFormat::Text => {
            eprintln!("error: {err}");
        }
    }
    code
}

/// Map an [`Error`] variant to its exit code. See the module-level
/// doc comment for the full contract.
fn exit_code_for(err: &Error) -> i32 {
    match err {
        Error::SpecifyVersionTooOld { .. } => EXIT_VERSION_TOO_OLD,
        Error::Validation { .. } => EXIT_VALIDATION_FAILED,
        _ => EXIT_GENERIC_FAILURE,
    }
}

/// Serialise a JSON payload with `schema-version` automatically set on
/// object-shaped responses.
fn emit_json(value: serde_json::Value) {
    let wrapped = match value {
        serde_json::Value::Object(mut map) => {
            map.entry("schema-version".to_string())
                .or_insert(serde_json::Value::from(JSON_SCHEMA_VERSION));
            serde_json::Value::Object(map)
        }
        other => other,
    };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("JSON serialise"));
}

fn emit_json_error(err: &Error, code: i32) {
    // Variant identifiers are rendered as kebab-case on the wire so the
    // `error` value matches the kebab-everywhere convention of the v2
    // JSON contract (RFC-2 §2). Single-word variants (`io`, `yaml`,
    // `config`, `merge`, `lifecycle`, `validation`) are unchanged.
    let variant = match err {
        Error::NotInitialized => "not-initialized",
        Error::SchemaResolution(_) => "schema-resolution",
        Error::Config(_) => "config",
        Error::Validation { .. } => "validation",
        Error::Merge(_) => "merge",
        Error::Lifecycle { .. } => "lifecycle",
        Error::SpecifyVersionTooOld { .. } => "specify-version-too-old",
        Error::PlanTransition { .. } => "plan-transition",
        Error::PlanHasOutstandingWork { .. } => "plan-has-outstanding-work",
        Error::DriverBusy { .. } => "driver-busy",
        Error::Io(_) => "io",
        Error::Yaml(_) => "yaml",
    };
    emit_json(json!({
        "error": variant,
        "message": err.to_string(),
        "exit-code": code,
    }));
}

// ---------------------------------------------------------------------------
// vectis dispatcher
// ---------------------------------------------------------------------------

/// Dispatch one of the four `specify vectis` verbs to the
/// `specify-vectis` library and translate the outcome into the v2
/// contract.
///
/// JSON output goes through [`emit_json`], which auto-injects
/// `schema-version: 2`. Text output is rendered per-verb by the
/// `vectis_text_render_*` helpers below: humanised summaries that match
/// the shapes documented in chunk 5 of
/// `docs/plans/fold-vectis-into-specify.md`. Error variants and the
/// synthesised `not-implemented` shape are kebab-case for JSON and
/// humanised for text.
fn run_vectis(format: OutputFormat, action: &VectisAction) -> i32 {
    let result = match action {
        VectisAction::Init(args) => specify_vectis::init::run(args),
        VectisAction::Verify(args) => specify_vectis::verify::run(args),
        VectisAction::AddShell(args) => specify_vectis::add_shell::run(args),
        VectisAction::UpdateVersions(args) => specify_vectis::update_versions::run(args),
    };
    match result {
        Ok(specify_vectis::CommandOutcome::Success(value)) => {
            match format {
                OutputFormat::Json => emit_json(value),
                OutputFormat::Text => vectis_render_text(action, &value),
            }
            EXIT_SUCCESS
        }
        Ok(specify_vectis::CommandOutcome::Stub { command }) => {
            let message = format!("`vectis {command}` is not implemented yet");
            match format {
                OutputFormat::Json => emit_json(json!({
                    "error": "not-implemented",
                    "command": command,
                    "message": message,
                    "exit-code": EXIT_GENERIC_FAILURE,
                })),
                OutputFormat::Text => eprintln!("error: {message}"),
            }
            EXIT_GENERIC_FAILURE
        }
        Err(err) => emit_vectis_error(format, &err),
    }
}

/// Render a [`specify_vectis::VectisError`] using the v2 contract:
/// kebab-case `error` variant, `message`, and the binary's mapped
/// `exit-code`. The text path renders each variant in a shape an
/// operator can act on without having to re-run with `--format json` —
/// notably, `MissingPrerequisites` lists each missing tool's `tool`,
/// `check`, and `install` on its own line.
///
/// We can't reuse [`emit_json_error`] because that helper is hard-coded
/// against the `specify_error::Error` enum; this is the vectis-shaped
/// sibling.
fn emit_vectis_error(format: OutputFormat, err: &specify_vectis::VectisError) -> i32 {
    let code = match err {
        specify_vectis::VectisError::MissingPrerequisites { .. } => EXIT_VALIDATION_FAILED,
        _ => EXIT_GENERIC_FAILURE,
    };
    match format {
        OutputFormat::Json => {
            // Single source of truth for the kebab-case `error` variant
            // and per-variant payload shape lives in
            // `VectisError::to_json`; we just splice in the dispatcher's
            // `exit-code` mapping on top so both callers (this helper
            // and any future direct caller of `to_json`) cannot drift.
            let mut payload = match err.to_json() {
                Value::Object(map) => map,
                _ => unreachable!("VectisError::to_json always returns an object"),
            };
            payload
                .entry("exit-code".to_string())
                .or_insert(Value::from(code));
            emit_json(Value::Object(payload));
        }
        OutputFormat::Text => match err {
            specify_vectis::VectisError::MissingPrerequisites { missing, message } => {
                eprintln!("error: missing prerequisites");
                for tool in missing {
                    eprintln!(
                        "  - {} ({}): {} | install: {}",
                        tool.tool, tool.assembly, tool.check, tool.install
                    );
                }
                eprintln!("{message}");
            }
            _ => {
                eprintln!("error: {err}");
            }
        },
    }
    code
}

// ---------------------------------------------------------------------------
// vectis text renderers
// ---------------------------------------------------------------------------

/// Dispatch a successful `vectis` payload to the per-verb text renderer.
///
/// The renderers consume the v2 JSON shape directly (rather than the
/// typed result) so this dispatcher does not have to re-thread the four
/// concrete success types out of the library and stays in lock-step
/// with the JSON contract by construction. Defensive `as_*` chains
/// fall back to empty strings/arrays so a future field addition does
/// not panic the text path.
fn vectis_render_text(action: &VectisAction, value: &Value) {
    match action {
        VectisAction::Init(_) => vectis_render_init_text(value),
        VectisAction::Verify(_) => vectis_render_verify_text(value),
        VectisAction::AddShell(_) => vectis_render_add_shell_text(value),
        VectisAction::UpdateVersions(_) => vectis_render_update_versions_text(value),
    }
}

fn vectis_render_init_text(value: &Value) {
    let app = value
        .get("app-name")
        .and_then(Value::as_str)
        .unwrap_or("<app>");
    let dir = value
        .get("project-dir")
        .and_then(Value::as_str)
        .unwrap_or("<dir>");
    println!("Created app \"{app}\" at {dir}");

    let caps: Vec<&str> = value
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if caps.is_empty() {
        println!("Capabilities: (none)");
    } else {
        println!("Capabilities: {}", caps.join(", "));
    }

    println!("Assemblies:");
    if let Some(map) = value.get("assemblies").and_then(Value::as_object) {
        // Preserve a stable order: core first, then ios, then android,
        // then anything else alphabetically. Matches the order users
        // see in the JSON envelope.
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| match k.as_str() {
            "core" => (0, String::new()),
            "ios" => (1, String::new()),
            "android" => (2, String::new()),
            other => (3, other.to_string()),
        });
        for key in keys {
            let assembly = &map[key];
            let status = assembly
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let file_count = assembly
                .get("files")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let build = vectis_render_build_steps_summary(assembly.get("build-steps"));
            match build {
                Some(summary) => println!("  - {key}: {status} ({file_count} files), {summary}"),
                None => println!("  - {key}: {status} ({file_count} files)"),
            }
        }
    }
}

fn vectis_render_verify_text(value: &Value) {
    let dir = value
        .get("project-dir")
        .and_then(Value::as_str)
        .unwrap_or("<dir>");
    let passed = value.get("passed").and_then(Value::as_bool).unwrap_or(false);
    println!(
        "Verified {dir}: {}",
        if passed { "PASS" } else { "FAIL" }
    );
    if let Some(map) = value.get("assemblies").and_then(Value::as_object) {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| match k.as_str() {
            "core" => (0, String::new()),
            "ios" => (1, String::new()),
            "android" => (2, String::new()),
            other => (3, other.to_string()),
        });
        for key in keys {
            let assembly = &map[key];
            let assembly_passed = assembly
                .get("passed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            println!(
                "  - {key}: {}",
                if assembly_passed { "PASS" } else { "FAIL" }
            );
            if !assembly_passed
                && let Some(steps) = assembly.get("steps").and_then(Value::as_array)
            {
                for step in steps {
                    let name = step.get("name").and_then(Value::as_str).unwrap_or("?");
                    let step_passed = step
                        .get("passed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    println!(
                        "      - {name}: {}",
                        if step_passed { "PASS" } else { "FAIL" }
                    );
                    if !step_passed
                        && let Some(err) = step.get("error").and_then(Value::as_str)
                        && let Some(first) = err.lines().find(|l| !l.trim().is_empty())
                    {
                        println!("        error: {first}");
                    }
                }
            }
        }
    }
}

fn vectis_render_add_shell_text(value: &Value) {
    let app = value
        .get("app-name")
        .and_then(Value::as_str)
        .unwrap_or("<app>");
    let dir = value
        .get("project-dir")
        .and_then(Value::as_str)
        .unwrap_or("<dir>");
    let platform = value
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or("<platform>");
    println!("Added {platform} shell to \"{app}\" at {dir}");

    let detected: Vec<&str> = value
        .get("detected-capabilities")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if detected.is_empty() {
        println!("Detected capabilities: (none)");
    } else {
        println!("Detected capabilities: {}", detected.join(", "));
    }
    let unrecognized: Vec<&str> = value
        .get("unrecognized-capabilities")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !unrecognized.is_empty() {
        println!("Unrecognized capabilities: {}", unrecognized.join(", "));
    }

    let assembly = value.get("assembly");
    let file_count = assembly
        .and_then(|a| a.get("files"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let build = vectis_render_build_steps_summary(assembly.and_then(|a| a.get("build-steps")));
    match build {
        Some(summary) => println!("Files: {file_count}, {summary}"),
        None => println!("Files: {file_count}"),
    }
}

fn vectis_render_update_versions_text(value: &Value) {
    let target = value
        .get("version-file")
        .and_then(Value::as_str)
        .unwrap_or("<file>");
    let dry_run = value.get("dry-run").and_then(Value::as_bool).unwrap_or(false);
    let written = value.get("written").and_then(Value::as_bool).unwrap_or(false);
    let mode = if dry_run {
        " (dry-run)"
    } else if written {
        " (written)"
    } else {
        " (no write)"
    };
    println!("Versions file: {target}{mode}");

    let changes = value
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if changes.is_empty() {
        println!("No changes.");
    } else {
        println!("Changes:");
        for c in &changes {
            let key = c.get("key").and_then(Value::as_str).unwrap_or("?");
            let cur = c.get("current").and_then(Value::as_str).unwrap_or("?");
            let prop = c.get("proposed").and_then(Value::as_str).unwrap_or("?");
            println!("  - {key}: {cur} → {prop}");
        }
    }

    if let Some(errors) = value.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        println!("Errors:");
        for e in errors {
            if let Some(s) = e.as_str() {
                println!("  - {s}");
            }
        }
    }

    if let Some(verification) = value.get("verification") {
        let passed = verification
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        println!(
            "Verify matrix: {}",
            if passed { "PASS" } else { "FAIL" }
        );
        if let Some(combos) = verification.get("combos").and_then(Value::as_array) {
            for combo in combos {
                let caps = combo.get("caps").and_then(Value::as_str).unwrap_or("?");
                let combo_passed = combo
                    .get("passed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                println!(
                    "  - {caps}: {}",
                    if combo_passed { "PASS" } else { "FAIL" }
                );
            }
        }
    }
}

/// Summarise a `build-steps` array (init/add-shell shapes) as either
/// "build PASS" or "build FAIL (<first failing step name>)". Returns
/// `None` when no `build-steps` field is present (e.g. the `core`
/// assembly entry from `init`).
fn vectis_render_build_steps_summary(steps: Option<&Value>) -> Option<String> {
    let arr = steps?.as_array()?;
    if arr.is_empty() {
        return Some("build PASS".to_string());
    }
    for step in arr {
        let passed = step.get("passed").and_then(Value::as_bool).unwrap_or(false);
        if !passed {
            let name = step.get("name").and_then(Value::as_str).unwrap_or("?");
            return Some(format!("build FAIL ({name})"));
        }
    }
    Some("build PASS".to_string())
}

fn absolute_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
