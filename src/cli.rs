use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use specify::{CreateIfExists, EntryKind, LifecycleStatus, Phase};
use specify_change::Status;

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
    /// Initialize .specify/ in a project.
    ///
    /// Pass `<capability>` (bare name or URL) for a regular project, or
    /// `--hub` for a registry-only platform hub. The two are mutually
    /// exclusive.
    Init {
        /// Capability identifier or URL (e.g. `omnia`,
        /// `https://github.com/<owner>/<repo>/capabilities/<name>`).
        /// Required unless `--hub` is set.
        capability: Option<String>,
        /// Project name (defaults to the project directory name)
        #[arg(long)]
        name: Option<String>,
        /// Project domain description (tech stack, architecture, testing)
        #[arg(long)]
        domain: Option<String>,
        /// Scaffold a registry-only platform hub instead of a regular
        /// project. Refuses to run when `.specify/` already exists.
        #[arg(long)]
        hub: bool,
    },

    /// Project dashboard — registry summary, plan progress, active changes
    Status,

    /// Refresh AGENTS.md and check whether generated context is current.
    Context {
        #[command(subcommand)]
        action: ContextAction,
    },

    /// Capability operations
    Capability {
        #[command(subcommand)]
        action: CapabilityAction,
    },

    /// Codex rule catalogue operations
    Codex {
        #[command(subcommand)]
        action: CodexAction,
    },

    /// WASI tool runner (RFC-15).
    Tool {
        #[command(subcommand)]
        action: ToolAction,
    },

    /// Slice lifecycle operations — one `define → build → merge` loop.
    Slice {
        #[command(subcommand)]
        action: SliceAction,
    },

    /// Change orchestration — operator brief, plan, finalize.
    Change {
        #[command(subcommand)]
        action: ChangeAction,
    },

    /// Platform registry at `registry.yaml` (repo root)
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// Materialise and manage registry peers under `.specify/workspace/`.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// One-shot layout migrations. All idempotent.
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },

    /// Generate shell completions for the given shell.
    #[command(hide = true)]
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Refreshable repository context for agent-facing guidance.
#[derive(Subcommand)]
pub enum ContextAction {
    /// Generate or refresh the managed `AGENTS.md` context block.
    Generate {
        /// Exit non-zero if AGENTS.md or the context lock would change; do not write.
        #[arg(long)]
        check: bool,
        /// Rewrite managed context despite unfenced or edited generated content.
        #[arg(long)]
        force: bool,
    },
    /// Check whether `AGENTS.md` matches current repository inputs.
    Check,
}

/// Umbrella `change` verbs — owns `change.md` and `plan.yaml`.
#[derive(Subcommand)]
pub enum ChangeAction {
    /// Scaffold `change.md` at the repo root. Refuses to overwrite.
    Create {
        /// Kebab-case change name (baked into the frontmatter).
        name: String,
    },
    /// Print the parsed change brief (text or JSON). Absent file exits 0.
    Show,
    /// Manage the change's executable plan (`plan.yaml`).
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Close out a change once every plan entry is terminal and every
    /// per-project PR has been operator-merged on its remote. Atomic:
    /// any guard failure leaves on-disk state untouched. Never merges
    /// PRs — operator lands them first through the forge.
    Finalize {
        /// Remove `.specify/workspace/<peer>/` clones after archiving.
        /// Refused when any clone has a dirty working tree.
        #[arg(long)]
        clean: bool,
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum ToolAction {
    /// Fetch if needed, then run a declared WASI tool.
    Run {
        /// Declared tool name.
        name: String,
        /// Args forwarded to the tool after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// List declared tools and cache status.
    List,
    /// Fetch one declared tool, or every declared tool when omitted.
    Fetch {
        /// Optional declared tool name to fetch.
        name: Option<String>,
    },
    /// Show one declared tool's metadata.
    Show {
        /// Declared tool name.
        name: String,
    },
    /// Remove unused cache entries for the current project.
    Gc {
        /// Scan every current-project tool scope. Currently equivalent to the default scan.
        #[arg(long)]
        all: bool,
    },
}

/// Plan-authoring verbs (`specify change plan *`).
#[derive(Subcommand)]
pub enum PlanAction {
    /// Scaffold an empty plan.yaml at the repo root
    Create {
        /// Kebab-case change name
        name: String,
        /// Named source, repeated: --source <key>=<path-or-url>
        #[arg(long = "source", value_parser = parse_source_kv)]
        sources: Vec<(String, String)>,
    },
    /// Validate plan.yaml (structure + plan/change consistency)
    Validate,
    /// Diagnose plan health (superset of `validate`). Adds
    /// `cycle-in-depends-on`, `orphan-source-key`, `stale-workspace-clone`,
    /// and `unreachable-entry` checks on top of `validate`.
    Doctor,
    /// Return the next eligible plan entry (respects depends-on + in-progress)
    Next,
    /// Show change progress report
    Status,
    /// Add a new plan entry (status: pending)
    Add {
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
        /// Plan-entry `schema` target for project-less entries (e.g. `contracts@v1`)
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
        /// Replace the plan-entry `schema` target. Pass `--schema ""` to clear;
        /// omit the flag to leave it unchanged.
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
        target: Status,
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
    /// Driver-lock primitives — `.specify/plan.lock` PID stamp used by
    /// `/change:execute` to serialise concurrent drivers.
    Lock {
        #[command(subcommand)]
        action: LockAction,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Create symlinks or git clones under `.specify/workspace/<name>/`.
    /// No-op when `registry.yaml` is absent.
    Sync {
        /// Specific project(s) to sync; omit to sync all registry projects.
        #[arg()]
        projects: Vec<String>,
    },
    /// Report slot materialisation, Git state, project config, and active slices per entry.
    Status {
        /// Specific project(s) to inspect; omit to inspect all registry projects.
        #[arg()]
        projects: Vec<String>,
    },
    /// Hidden executor helper: prepare one workspace slot on `specify/<change>`.
    #[command(hide = true)]
    PrepareBranch {
        /// Registry project to prepare.
        project: String,
        /// Kebab-case umbrella change name.
        #[arg(long)]
        change: String,
        /// Active entry source path allowed to be dirty during resume.
        #[arg(long = "source", value_name = "PATH")]
        sources: Vec<PathBuf>,
        /// Capability-owned output path allowed to be dirty during resume.
        #[arg(long = "output", value_name = "PATH")]
        outputs: Vec<PathBuf>,
    },
    /// Push workspace clones to their remote repositories.
    Push {
        /// Specific project(s) to push; omit to push all dirty clones.
        #[arg()]
        projects: Vec<String>,
        /// Show what would happen without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Deprecated: automated PR merge was removed. Compatibility shim
    /// that exits non-zero. Use `gh pr merge` then `specify change finalize`.
    Merge {
        /// Accepted for compatibility; ignored.
        #[arg()]
        projects: Vec<String>,
        /// Accepted for compatibility; ignored.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum LockAction {
    /// Acquire the plan.lock PID stamp. Fails when another live PID holds
    /// it; stale stamps are reclaimed silently.
    Acquire {
        /// PID to stamp; defaults to `std::process::id()`.
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Release the stamp when we hold it. Refuses to clobber another PID's.
    Release {
        /// PID that expects to own the stamp; defaults to `std::process::id()`.
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Report the current lock state (holder PID, stale flag).
    Status,
}

/// Registry operations on `registry.yaml`.
#[derive(Subcommand)]
pub enum RegistryAction {
    /// Print the parsed `registry.yaml` (text or JSON). Absent file exits 0.
    Show,
    /// Validate `registry.yaml` shape. Absent file exits 0.
    Validate,
    /// Append a new project entry to `registry.yaml`. Creates the file
    /// when absent.
    Add {
        /// Kebab-case project name. Must be unique within the registry.
        name: String,
        /// Clone target — `.`, a repo-relative path, `git@host:path`, or
        /// `http(s)://` / `ssh://` / `git+...` remote.
        #[arg(long)]
        url: String,
        /// Capability identifier (e.g. `omnia@v1`). Non-empty after trim.
        #[arg(long)]
        schema: String,
        /// Domain-level characterisation; required when the registry
        /// declares more than one project.
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove an existing project entry. Warns when `plan.yaml` references it.
    Remove {
        /// Kebab-case project name to remove.
        name: String,
    },
}

#[derive(Subcommand)]
pub enum MigrateAction {
    /// Move v1 layout artifacts (`registry.yaml`, `plan.yaml`,
    /// `initiative.md`, `contracts/`) from `.specify/` to the repo root.
    /// Refuses to clobber existing destinations or run inside a workspace
    /// clone.
    V2Layout {
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum CapabilityAction {
    /// Resolve a capability value to a directory path
    Resolve {
        /// Capability value (bare name or URL) to resolve through the
        /// project-local cache and bundled capability lookup
        capability_value: String,
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,
    },
    /// Validate a `capability.yaml` file.
    Check {
        /// Directory containing `capability.yaml`
        capability_dir: PathBuf,
    },
    /// List the briefs for a phase in topological order (optionally
    /// with completion status against a specific slice)
    Pipeline {
        /// Pipeline phase to enumerate
        #[arg(value_enum)]
        phase: Phase,
        /// Slice directory; when supplied, each brief includes a
        /// `present` boolean reflecting whether its `generates`
        /// artifact exists under the directory
        #[arg(long)]
        slice: Option<PathBuf>,
    },
}

/// Project-resolved codex rule catalogue verbs.
#[derive(Subcommand)]
pub enum CodexAction {
    /// List resolved codex rules.
    List,
    /// Show one resolved codex rule.
    Show {
        /// Stable codex rule id, e.g. `UNI-002`.
        rule_id: String,
    },
    /// Validate the resolved codex rule set.
    Validate,
    /// Export the resolved codex as JSON.
    Export,
}

#[derive(Subcommand)]
pub enum SliceAction {
    /// Create a new slice directory with an initial `.metadata.yaml`
    Create {
        /// Kebab-case slice name
        name: String,
        /// Capability identifier; defaults to the value in `.specify/project.yaml`
        #[arg(long)]
        schema: Option<String>,
        /// Behaviour when `<slices_dir>/<name>/` already exists
        #[arg(long, value_enum, default_value = "fail")]
        if_exists: CreateIfExistsArg,
    },
    /// List every active slice under `.specify/slices/`
    List,
    /// Show the status of one slice
    Status {
        /// Slice name (under `.specify/slices/`)
        name: String,
    },
    /// Validate a slice's artifacts against capability validation rules
    Validate {
        /// Slice name (under `.specify/slices/`)
        name: String,
    },
    /// Spec-merge operations for a slice
    Merge {
        #[command(subcommand)]
        action: SliceMergeAction,
    },
    /// Tasks-list operations for a slice
    Task {
        #[command(subcommand)]
        action: SliceTaskAction,
    },
    /// Phase-outcome bookkeeping on `.metadata.yaml`
    Outcome {
        #[command(subcommand)]
        action: OutcomeAction,
    },
    /// Append-only audit log at `<slice_dir>/journal.yaml`
    Journal {
        #[command(subcommand)]
        action: JournalAction,
    },
    /// Transition a slice to a new lifecycle status
    Transition {
        /// Slice name
        name: String,
        /// Target status (`defined`, `building`, `complete`, `merged`, `dropped`, or `defining`)
        #[arg(value_enum)]
        target: LifecycleStatus,
    },
    /// Scan or overwrite `touched_specs` on `.metadata.yaml`
    TouchedSpecs {
        /// Slice name
        name: String,
        /// Scan `specs/` subdirs and classify each as new or modified
        #[arg(long, conflicts_with = "set")]
        scan: bool,
        /// Replace `touched_specs` with the listed capabilities (each `<name>:new|modified`)
        #[arg(long, value_delimiter = ',')]
        set: Vec<String>,
    },
    /// Report overlapping `touched_specs` with other active slices
    Overlap {
        /// Slice name
        name: String,
    },
    /// Archive a slice directory into `.specify/archive/YYYY-MM-DD-<name>/`
    Archive {
        /// Slice name
        name: String,
    },
    /// Transition a slice to `dropped` and archive it
    Drop {
        /// Slice name
        name: String,
        /// Free-text reason; surfaced in `.metadata.yaml.drop_reason` and the archive path
        #[arg(long)]
        reason: Option<String>,
    },
}

/// Spec-merge subcommands grouped under `slice merge`.
#[derive(Subcommand)]
pub enum SliceMergeAction {
    /// Merge all delta specs for the slice into baseline and archive the slice
    Run {
        /// Slice name
        name: String,
    },
    /// Show the merge operations that would be applied, without writing
    Preview {
        /// Slice name
        name: String,
    },
    /// Report `type: modified` baselines modified after this slice's `defined_at`
    ConflictCheck {
        /// Slice name
        name: String,
    },
}

/// Task-list subcommands grouped under `slice task`.
#[derive(Subcommand)]
pub enum SliceTaskAction {
    /// Report task completion counts (total, complete, pending)
    Progress {
        /// Slice name
        name: String,
    },
    /// Mark a task complete (idempotent — no-op if already complete)
    Mark {
        /// Slice name
        name: String,
        /// Task number (e.g. `1.1`)
        task_number: String,
    },
}

/// Phase-outcome subcommands grouped under `slice outcome`.
#[derive(Subcommand)]
pub enum OutcomeAction {
    /// Record the outcome of a phase (define|build|merge) on `.metadata.yaml`
    Set {
        /// Slice name
        name: String,
        /// Phase this outcome applies to
        #[arg(value_enum)]
        phase: Phase,
        /// Outcome classification. `registry-amendment-required` requires
        /// the four `--proposed-*` flags plus `--rationale`.
        #[arg(value_enum)]
        outcome: OutcomeKind,
        /// Short explanation of what happened. Optional for
        /// `registry-amendment-required` (synthesised from `--proposed-name`).
        #[arg(long)]
        summary: Option<String>,
        /// Optional verbatim detail (stderr, ambiguous-requirement text, etc.)
        #[arg(long)]
        context: Option<String>,
        /// Proposed kebab-case project name. Required when
        /// `<outcome>` is `registry-amendment-required`.
        #[arg(long)]
        proposed_name: Option<String>,
        /// Proposed clone URL. Required when `<outcome>` is
        /// `registry-amendment-required`.
        #[arg(long)]
        proposed_url: Option<String>,
        /// Proposed capability identifier (e.g. `omnia@v1`). Required when
        /// `<outcome>` is `registry-amendment-required`.
        #[arg(long)]
        proposed_schema: Option<String>,
        /// Optional human-readable description of the proposed project.
        #[arg(long)]
        proposed_description: Option<String>,
        /// Rationale prose. Required when `<outcome>` is
        /// `registry-amendment-required`.
        #[arg(long)]
        rationale: Option<String>,
    },
    /// Read the stamped `.metadata.yaml.outcome` for a slice. Exits 0
    /// whether or not an outcome has been stamped.
    Show {
        /// Slice name
        name: String,
    },
}

/// CLI-side discriminant for `slice outcome set <outcome>`. Mirrors
/// the on-disk [`specify::Outcome`] kebab-case discriminants. Adding a
/// variant requires extending [`specify::Outcome`] too.
#[derive(Copy, Clone, ValueEnum, PartialEq, Eq, Debug)]
pub enum OutcomeKind {
    /// Phase completed successfully.
    Success,
    /// Phase failed.
    Failure,
    /// Phase deferred (needs human input).
    Deferred,
    /// Phase blocked on a registry amendment. Requires the
    /// `--proposed-name` / `--proposed-url` / `--proposed-schema` /
    /// `--rationale` flags.
    RegistryAmendmentRequired,
}

/// Journal subcommands grouped under `slice journal`.
#[derive(Subcommand)]
pub enum JournalAction {
    /// Append an entry to the slice's `journal.yaml`
    Append {
        /// Slice name
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
    /// Print the slice's journal entries (text or JSON)
    Show {
        /// Slice name
        name: String,
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
