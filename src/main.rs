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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use specify::{
    BaselineConflict, Brief, ChangeMetadata, CreateIfExists, CreateOutcome, Error, InitOptions,
    InitResult, LifecycleStatus, MergeEntry, MergeOperation, MergeResult, Overlap, Phase,
    PipelineView, ProjectConfig, Schema, SchemaSource, SpecType, Task, TouchedSpec,
    ValidationReport, ValidationResult, VersionMode, change_actions, conflict_check, init,
    mark_complete, merge_change, parse_tasks, preview_change, serialize_report, validate_change,
};

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_GENERIC_FAILURE: i32 = 1;
pub const EXIT_VALIDATION_FAILED: i32 = 2;
pub const EXIT_VERSION_TOO_OLD: i32 = 3;

/// JSON contract version emitted on every structured response. Bumping
/// this field is a breaking change for skill authors — see RFC-1
/// §"JSON Contract Versioning".
const JSON_SCHEMA_VERSION: u64 = 1;

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
        phase: PhaseArg,
        /// Change directory; when supplied, each brief includes a
        /// `present` boolean reflecting whether its `generates`
        /// artifact exists under the directory
        #[arg(long)]
        change: Option<PathBuf>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum PhaseArg {
    Define,
    Build,
    Merge,
}

impl From<PhaseArg> for Phase {
    fn from(value: PhaseArg) -> Self {
        match value {
            PhaseArg::Define => Phase::Define,
            PhaseArg::Build => Phase::Build,
            PhaseArg::Merge => Phase::Merge,
        }
    }
}

impl PhaseArg {
    fn as_str(self) -> &'static str {
        match self {
            PhaseArg::Define => "define",
            PhaseArg::Build => "build",
            PhaseArg::Merge => "merge",
        }
    }
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
        target: LifecycleStatusArg,
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

#[derive(Copy, Clone, ValueEnum)]
enum LifecycleStatusArg {
    Defining,
    Defined,
    Building,
    Complete,
    Merged,
    Dropped,
}

impl From<LifecycleStatusArg> for LifecycleStatus {
    fn from(value: LifecycleStatusArg) -> Self {
        match value {
            LifecycleStatusArg::Defining => LifecycleStatus::Defining,
            LifecycleStatusArg::Defined => LifecycleStatus::Defined,
            LifecycleStatusArg::Building => LifecycleStatus::Building,
            LifecycleStatusArg::Complete => LifecycleStatus::Complete,
            LifecycleStatusArg::Merged => LifecycleStatus::Merged,
            LifecycleStatusArg::Dropped => LifecycleStatus::Dropped,
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
                "config_path": absolute_string(&result.config_path),
                "schema_name": result.schema_name,
                "cache_present": result.cache_present,
                "directories_created": result.directories_created
                    .iter()
                    .map(|p| absolute_string(p))
                    .collect::<Vec<_>>(),
                "scaffolded_rule_keys": result.scaffolded_rule_keys,
                "specify_version": result.specify_version,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
                "merged_specs": specs,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
                "change_dir": change_dir.display().to_string(),
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
        "baseline_path": entry.baseline_path.display().to_string(),
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
                "change_dir": change_dir.display().to_string(),
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
        "defined_at": c.defined_at,
        "baseline_modified_at": c.baseline_modified_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
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
            "old_name": old_name,
            "new_name": new_name,
        }),
        MergeOperation::CreatedBaseline { requirement_count } => json!({
            "kind": "created_baseline",
            "requirement_count": requirement_count,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
        "skill_directive": skill,
    })
}

fn run_task_mark(format: OutputFormat, change_dir: PathBuf, task_number: String) -> i32 {
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
                "new_content_path": tasks_path.display().to_string(),
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
            "schema_value": schema_value,
            "resolved_path": path.display().to_string(),
            "source": source,
        })),
        OutputFormat::Text => println!("{}", path.display()),
    }
    EXIT_SUCCESS
}

fn run_schema_pipeline(format: OutputFormat, phase_arg: PhaseArg, change: Option<PathBuf>) -> i32 {
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
        Err(err) => return emit_error(format, &err),
    };
    let pipeline = match PipelineView::load(&config.schema, &project_dir) {
        Ok(view) => view,
        Err(err) => return emit_error(format, &err),
    };

    let phase: Phase = phase_arg.into();
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
                "phase": phase_arg.as_str(),
                "change": change.as_ref().map(|p| p.display().to_string()),
                "briefs": briefs,
            }));
        }
        OutputFormat::Text => {
            println!("phase: {}", phase_arg.as_str());
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
            "rule_id": rule_id,
            "rule": rule,
        }),
        ValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => json!({
            "status": "fail",
            "rule_id": rule_id,
            "rule": rule,
            "detail": detail,
        }),
        ValidationResult::Deferred {
            rule_id,
            rule,
            reason,
        } => json!({
            "status": "deferred",
            "rule_id": rule_id,
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
        ChangeAction::Transition { name, target } => {
            run_change_transition(format, name, target.into())
        }
        ChangeAction::TouchedSpecs { name, scan, set } => {
            run_change_touched_specs(format, name, scan, set)
        }
        ChangeAction::Overlap { name } => run_change_overlap(format, name),
        ChangeAction::Archive { name } => run_change_archive(format, name),
        ChangeAction::Drop { name, reason } => run_change_drop(format, name, reason),
    }
}

fn run_change_create(
    format: OutputFormat, name: String, schema: Option<String>, if_exists: CreateIfExists,
) -> i32 {
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
            "change_dir": outcome.change_dir.display().to_string(),
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
            "defined_at": metadata.defined_at,
            "build_started_at": metadata.build_started_at,
            "completed_at": metadata.completed_at,
            "merged_at": metadata.merged_at,
            "dropped_at": metadata.dropped_at,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
            "touched_specs": touched_specs_to_json(&entries),
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
            "archive_path": target.display().to_string(),
        })),
        OutputFormat::Text => {
            println!("{name}: archived to {}", target.display());
        }
    }
    EXIT_SUCCESS
}

fn run_change_drop(format: OutputFormat, name: String, reason: Option<String>) -> i32 {
    let project_dir = match current_dir() {
        Ok(dir) => dir,
        Err(err) => return emit_error(format, &err),
    };
    let _config = match ProjectConfig::load(&project_dir) {
        Ok(cfg) => cfg,
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
            "archive_path": archive_path.display().to_string(),
            "drop_reason": metadata.drop_reason,
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

fn overlap_to_json(o: &Overlap) -> Value {
    json!({
        "capability": o.capability,
        "other_change": o.other_change,
        "our_spec_type": spec_type_label(o.our_spec_type),
        "other_spec_type": spec_type_label(o.other_spec_type),
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
// Shared helpers
// ---------------------------------------------------------------------------

fn current_dir() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(Error::Io)
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

/// Serialise a JSON payload with `schema_version` automatically set on
/// object-shaped responses.
fn emit_json(value: serde_json::Value) {
    let wrapped = match value {
        serde_json::Value::Object(mut map) => {
            map.entry("schema_version".to_string())
                .or_insert(serde_json::Value::from(JSON_SCHEMA_VERSION));
            serde_json::Value::Object(map)
        }
        other => other,
    };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("JSON serialise"));
}

fn emit_json_error(err: &Error, code: i32) {
    let variant = match err {
        Error::NotInitialized => "not_initialized",
        Error::SchemaResolution(_) => "schema_resolution",
        Error::Config(_) => "config",
        Error::Validation { .. } => "validation",
        Error::Merge(_) => "merge",
        Error::Lifecycle { .. } => "lifecycle",
        Error::SpecifyVersionTooOld { .. } => "specify_version_too_old",
        Error::PlanTransition { .. } => "plan_transition",
        Error::PlanHasOutstandingWork { .. } => "plan_has_outstanding_work",
        Error::Io(_) => "io",
        Error::Yaml(_) => "yaml",
    };
    emit_json(json!({
        "error": variant,
        "message": err.to_string(),
        "exit_code": code,
    }));
}

fn absolute_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
