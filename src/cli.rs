use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use specify::{CreateIfExists, EntryKind, LifecycleStatus, Outcome, Phase, PlanStatus};

#[derive(Parser)]
#[command(
    name = "specify",
    version,
    about = "Specify CLI — deterministic operations for spec-driven development"
)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Output format
    #[arg(long, default_value = "text", global = true)]
    pub(crate) format: OutputFormat,
}

#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
pub enum Commands {
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
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },

    /// Initiative metadata: operator brief and platform registry.
    Initiative {
        #[command(subcommand)]
        action: InitiativeAction,
    },

    /// Materialise and manage registry peers under `.specify/workspace/` (RFC-3a/3b).
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Generate shell completions for the given shell.
    #[command(hide = true)]
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
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
    /// reports back as [`CliResult::ValidationFailed`] (`2`) — locally
    /// "your workstation is incomplete", which slots cleanly into the
    /// existing "validation failed" bucket — and every other failure
    /// returns [`CliResult::GenericFailure`] (`1`).
    Vectis {
        #[command(subcommand)]
        action: VectisAction,
    },
}

/// Subcommands under `specify vectis`. Each variant flattens the
/// matching `clap::Args` struct from the `specify-vectis` library so
/// flag parsing stays in lock-step with the library definition.
#[derive(Subcommand)]
pub enum VectisAction {
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
pub enum PlanAction {
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
        /// Named source keys (repeatable). Every value is a key in the top-level
        /// `sources` map.
        #[arg(long = "sources", action = ArgAction::Append)]
        sources: Vec<String>,
        /// Free-text scoping hint for the define step
        #[arg(long)]
        description: Option<String>,
        /// Target registry project name (RFC-3b)
        #[arg(long)]
        project: Option<String>,
        /// Schema identifier for project-less entries (e.g. `contracts@v1`)
        #[arg(long)]
        schema: Option<String>,
        /// Baseline paths relevant to this change, relative to `.specify/` (repeatable)
        #[arg(long)]
        context: Vec<String>,
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
        /// Replace sources. Pass `--sources` (with no value) to clear the field;
        /// omit the flag to leave it unchanged.
        #[arg(long = "sources", num_args = 0.., value_delimiter = ',')]
        sources: Option<Vec<String>>,
        /// Replace description. Pass `--description ""` to clear; omit the flag
        /// to leave it unchanged.
        #[arg(long)]
        description: Option<String>,
        /// Replace project. Pass `--project ""` to clear; omit the flag to leave it unchanged.
        #[arg(long)]
        project: Option<String>,
        /// Replace schema. Pass `--schema ""` to clear; omit the flag to leave it unchanged.
        #[arg(long)]
        schema: Option<String>,
        /// Replace context paths. Pass `--context` (with no value) to clear; omit the
        /// flag to leave it unchanged.
        #[arg(long, num_args = 0.., value_delimiter = ',')]
        context: Option<Vec<String>>,
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
pub enum InitiativeAction {
    /// Registry operations (RFC-3a §"The Registry").
    ///
    /// `.specify/registry.yaml` is the platform-level catalogue of peer
    /// projects. It's optional: an absent file is equivalent to single-repo
    /// mode. These verbs expose the shape-validation already used by
    /// `plan validate` as dedicated read/validate entry points.
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// Initiative brief operations (RFC-3a §"The Initiative Brief").
    ///
    /// `.specify/initiative.md` is the operator-authored brief: a YAML
    /// frontmatter block (`name`, optional `inputs`) plus free-form
    /// markdown body. It's optional — `init` scaffolds a canonical
    /// template; `show` prints the parsed brief.
    Brief {
        #[command(subcommand)]
        action: BriefAction,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Create symlinks or git clones under `.specify/workspace/<name>/`.
    ///
    /// No-op with exit 0 when `.specify/registry.yaml` is absent. Updates
    /// `.gitignore` to ignore `.specify/workspace/` when a registry exists.
    Sync,
    /// Report symlink vs git clone, `HEAD`, and dirty working tree per entry.
    Status,
    /// Push workspace clones to their remote repositories (RFC-3b).
    Push {
        /// Specific project(s) to push; omit to push all dirty clones.
        #[arg()]
        projects: Vec<String>,
        /// Show what would happen without making changes.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum LockAction {
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
pub enum RegistryAction {
    /// Print the parsed `.specify/registry.yaml` (text or JSON).
    ///
    /// Prints a clear "no registry declared" message when the file is
    /// absent (exit 0). Malformed files fail loud with a non-zero exit —
    /// the operator asked to show something unparseable.
    Show,
    /// Validate `.specify/registry.yaml` shape. Non-zero exit on any error.
    ///
    /// Absent registry is not an error: exit 0 with a "none declared"
    /// message. Well-formed registry exits 0. Malformed registry exits
    /// with `CliResult::ValidationFailed` and a diagnostic that names
    /// `registry.yaml`.
    Validate,
}

#[derive(Subcommand)]
pub enum BriefAction {
    /// Scaffold `.specify/initiative.md` from the canonical template.
    ///
    /// Refuses to overwrite an existing file — mirrors the
    /// `initiative init` posture for `plan.yaml`.
    Init {
        /// Kebab-case initiative name (baked into the frontmatter).
        name: String,
    },
    /// Print the parsed `.specify/initiative.md` (text or JSON).
    ///
    /// Absent file is not an error: exit 0 with "no initiative brief
    /// declared". Malformed file fails loud with a non-zero exit — the
    /// operator asked to show something unparseable.
    Show,
}

#[derive(Subcommand)]
pub enum SpecAction {
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
pub enum TaskAction {
    /// Report task completion counts (total, complete, pending)
    Progress { change_dir: PathBuf },
    /// Mark a task complete (idempotent — no-op if already complete)
    Mark { change_dir: PathBuf, task_number: String },
}

#[derive(Subcommand)]
pub enum SchemaAction {
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
pub enum ChangeAction {
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
pub enum CreateIfExistsArg {
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
            CreateIfExistsArg::Fail => Self::Fail,
            CreateIfExistsArg::Continue => Self::Continue,
            CreateIfExistsArg::Restart => Self::Restart,
        }
    }
}

/// Parse a single `--source <key>=<path-or-url>` CLI value into a
/// `(key, value)` pair. Returns a `String` error on malformed input so
/// clap surfaces a standard usage diagnostic (exit code 2).
pub fn parse_source_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("--source must be <key>=<path-or-url>, got `{s}`"))?;
    if k.is_empty() || v.is_empty() {
        return Err(format!("--source key and value must be non-empty, got `{s}`"));
    }
    Ok((k.to_string(), v.to_string()))
}
