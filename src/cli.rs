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
    /// Run `specify init <capability>` for a regular project (e.g.
    /// `specify init omnia` or `specify init https://...`), or
    /// `specify init --hub` for a registry-only platform hub. The
    /// `<capability>` positional and `--hub` are mutually exclusive;
    /// running with neither (or with both) errors with the
    /// `init-requires-capability-or-hub` diagnostic.
    Init {
        /// Capability identifier or URL to resolve before scaffolding
        /// (e.g. `omnia`, `https://github.com/<owner>/<repo>/capabilities/<name>`).
        /// Required unless `--hub` is set; mutually exclusive with `--hub`.
        capability: Option<String>,
        /// Project name (defaults to the project directory name)
        #[arg(long)]
        name: Option<String>,
        /// Project domain description (tech stack, architecture, testing)
        #[arg(long)]
        domain: Option<String>,
        /// Scaffold a registry-only **platform hub** (RFC-9 §1D)
        /// instead of a regular project: writes `registry.yaml` at
        /// the repo root and `project.yaml { hub: true }` (with
        /// `capability:` omitted — RFC-13 §Migration "Hub project
        /// shape") under `.specify/`. The change brief (`change.md`)
        /// and `plan.yaml` stay operator-managed (use
        /// `specify change create` / `specify change plan create`).
        /// Refuses to run when `.specify/` already exists. Mutually
        /// exclusive with the `<capability>` positional.
        #[arg(long)]
        hub: bool,
    },

    /// Project dashboard — registry summary, plan progress, active changes
    Status,

    /// Capability operations
    Capability {
        #[command(subcommand)]
        action: CapabilityAction,
    },

    /// WASI tool runner (RFC-15).
    Tool {
        #[command(subcommand)]
        action: ToolAction,
    },

    /// Slice lifecycle operations (the per-loop unit of work).
    ///
    /// A "slice" is the unit a single `define → build → merge` loop
    /// drives end to end (RFC-13 §"What becomes a capability").
    Slice {
        #[command(subcommand)]
        action: SliceAction,
    },

    /// Change orchestration — operator brief, plan, finalize.
    ///
    /// The umbrella verb family for an operator-defined outcome that
    /// coordinates one or more slices (RFC-13 §"What becomes a
    /// capability"). Owns the change brief at `change.md` and the
    /// `plan.yaml` that drives multi-slice execution.
    Change {
        #[command(subcommand)]
        action: ChangeAction,
    },

    /// Platform registry at `registry.yaml` (repo root)
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// Materialise and manage registry peers under `.specify/workspace/` (RFC-3a/3b).
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// One-shot layout migrations.
    ///
    /// Three subcommands:
    ///
    /// - `v2-layout` (RFC-9 §1B / RFC-13 chunk 2.0) moves the
    ///   operator-facing platform artifacts (`registry.yaml`,
    ///   `plan.yaml`, `initiative.md`, `contracts/`) from the legacy
    ///   v1 location under `.specify/` to the repo root.
    /// - `slice-layout` (RFC-13 chunk 3.6) renames `.specify/changes/`
    ///   to `.specify/slices/` on disk and rewrites any in-tree
    ///   `$CHANGE_DIR` substitutions in vendored skill markdown to
    ///   `$SLICE_DIR`. Refuses to run when a per-loop unit is
    ///   mid-phase (operator must finish or drop the in-progress
    ///   slice first).
    /// - `change-noun` (RFC-13 chunk 3.7) renames the umbrella
    ///   operator brief from `initiative.md` to `change.md` at the
    ///   repo root. No on-disk changes to other platform artifacts
    ///   (`registry.yaml`, `plan.yaml`, `contracts/` stay put per
    ///   RFC-9 §1B).
    ///
    /// All commands are idempotent — re-running on an already-
    /// migrated project exits 0 with a "nothing to migrate" message.
    /// `v2-layout` additionally refuses to run inside a workspace
    /// clone (`.specify/workspace/<name>/`); migrate the hub repo
    /// first, then iterate clones explicitly.
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

/// Operator-facing **change** verbs (RFC-13 §"What becomes a capability").
///
/// `change` is the umbrella orchestration noun: it holds the operator
/// brief (`change.md` at the repo root) and the executable plan (`plan.yaml`)
/// that drives one or more slices through `define → build → merge`.
///
/// The `Plan { action }` variant nests every plan-authoring sub-verb
/// under `specify change plan *` so the durable post-RFC surface reads
/// `specify change {create, plan {add,amend,next,status,doctor,lock,
/// transition,archive,validate,create}, finalize}`.
#[derive(Subcommand)]
pub enum ChangeAction {
    /// Scaffold the change brief (`change.md` at the repo root)
    /// from the canonical template.
    ///
    /// Refuses to overwrite an existing file — mirrors the
    /// `change plan create` posture for `plan.yaml`.
    Create {
        /// Kebab-case change name (baked into the frontmatter).
        name: String,
    },
    /// Print the parsed change brief (text or JSON).
    ///
    /// Absent file is not an error: exit 0 with "no change brief
    /// declared". Malformed file fails loud with a non-zero exit — the
    /// operator asked to show something unparseable.
    Show,
    /// Manage the change's executable plan (`plan.yaml` at the repo root).
    ///
    /// The plan-authoring sub-resource that drives slice execution.
    /// Verbs (`create`, `add`, `amend`, `next`, `status`, `doctor`,
    /// `lock`, `transition`, `archive`, `validate`) are unchanged from
    /// the previous top-level `specify plan *` family — they are now
    /// scoped under the change umbrella.
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Close out a change once every plan entry is in a terminal
    /// state and every per-project PR has been operator-merged on its
    /// remote (RFC-9 §4C / RFC-14 C09). Sweeps `plan.yaml`, the change brief, and the
    /// `.specify/plans/<name>/` authoring trail into
    /// `.specify/archive/plans/<YYYYMMDD>-<name>/`. With `--clean`
    /// also removes `.specify/workspace/<peer>/` clones.
    ///
    /// Atomic: any guard failure (non-terminal entry, unmerged PR,
    /// dirty workspace clone) refuses with a per-project status table
    /// and leaves the on-disk state untouched. The archive write
    /// preflights both destinations before any move, so a collision
    /// here also leaves the working tree alone.
    ///
    /// Finalize never merges PRs. The operator lands them first
    /// through the forge UI or `gh pr merge`, then re-runs finalize
    /// after clearing any failing guard.
    Finalize {
        /// Remove `.specify/workspace/<peer>/` clones after the archive
        /// completes. Refused when any clone has a dirty working tree
        /// (the diagnostic flags that `--clean` would drop the
        /// uncommitted work).
        #[arg(long)]
        clean: bool,
        /// Show what would happen without writing anything. Never
        /// invokes a forge merge command and never moves files.
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
///
/// `plan.yaml` (at the repo root) is the change's executable plan.
/// These verbs scope, validate, advance, and archive plan entries.
/// The shape is preserved from the previous top-level `Commands::Plan`
/// — it now nests under `Commands::Change` per RFC-13 §"What becomes a
/// capability".
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
    /// Diagnose plan health (superset of `validate`, RFC-9 §4B).
    ///
    /// Runs every check `plan validate` runs, then layers four
    /// additional health diagnostics on top:
    ///
    /// - `cycle-in-depends-on` — dependency cycles in `depends-on`,
    ///   reported with the cycle path. `next_eligible` silently skips
    ///   cycles at runtime; doctor is the only place where the cycle
    ///   structure is surfaced to the operator.
    /// - `orphan-source-key` — top-level `sources:` keys that no entry
    ///   references (the inverse of validate's `unknown-source`).
    /// - `stale-workspace-clone` — `.specify/workspace/<project>/`
    ///   slots whose materialisation no longer matches `registry.yaml`.
    /// - `unreachable-entry` — pending entries whose dependency
    ///   closure is rooted in a `failed` or `skipped` predecessor.
    ///
    /// Existing `plan validate` codes (`dependency-cycle`,
    /// `unknown-source`, etc.) are passed through unchanged so doctor
    /// is a strict superset.
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
pub enum WorkspaceAction {
    /// Create symlinks or git clones under `.specify/workspace/<name>/`.
    ///
    /// No-op with exit 0 when `registry.yaml` is absent. Updates
    /// `.gitignore` to ignore `.specify/workspace/` when a registry exists.
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
    /// Push workspace clones to their remote repositories (RFC-3b).
    Push {
        /// Specific project(s) to push; omit to push all dirty clones.
        #[arg()]
        projects: Vec<String>,
        /// Show what would happen without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Deprecated: automated PR merge was removed by RFC-14.
    ///
    /// One-release compatibility shim. Accepts the old arguments, then
    /// exits non-zero without reading registry state, looking up PRs, or
    /// performing any forge side effect. Merge PRs through the forge UI
    /// or `gh pr merge`, then run `specify change finalize`.
    Merge {
        /// Accepted for one-release compatibility; ignored by the shim.
        #[arg()]
        projects: Vec<String>,
        /// Accepted for one-release compatibility; ignored by the shim.
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

/// Registry operations (RFC-3a §"The Registry", RFC-9 §2A).
///
/// `registry.yaml` (at the repo root) is the platform-level catalogue
/// of peer projects. It's optional: an absent file is equivalent to
/// single-repo mode. These verbs expose the shape-validation already
/// used by `plan validate` as dedicated read/validate entry points,
/// plus the dynamic `add`/`remove` mutators introduced by RFC-9 §2A
/// so the operator no longer has to hand-edit the file.
#[derive(Subcommand)]
pub enum RegistryAction {
    /// Print the parsed `registry.yaml` (text or JSON).
    ///
    /// Prints a clear "no registry declared" message when the file is
    /// absent (exit 0). Malformed files fail loud with a non-zero exit —
    /// the operator asked to show something unparseable.
    Show,
    /// Validate `registry.yaml` shape. Non-zero exit on any error.
    ///
    /// Absent registry is not an error: exit 0 with a "none declared"
    /// message. Well-formed registry exits 0. Malformed registry exits
    /// with `CliResult::ValidationFailed` and a diagnostic that names
    /// `registry.yaml`.
    Validate,
    /// Append a new project entry to `registry.yaml`
    /// (RFC-9 §2A).
    ///
    /// Creates the file with `version: 1` when absent, validates the
    /// candidate shape with `Registry::validate_shape` (or
    /// `validate_shape_hub` when the project is a registry-only hub),
    /// and persists the result. Refuses to add a project that already
    /// exists. Surfaces the `description-missing-multi-repo`
    /// diagnostic when the addition produces a multi-project registry
    /// and any existing entry lacks a `description`.
    Add {
        /// Kebab-case project name. Must be unique within the registry.
        name: String,
        /// Clone target — `.`, a repo-relative path, `git@host:path`,
        /// or an `http(s)://`, `ssh://`, or `git+http(s)://` /
        /// `git+ssh://` remote. Validated by the same shape rules
        /// `registry validate` enforces.
        #[arg(long)]
        url: String,
        /// Capability identifier stored in registry.yaml's `schema:`
        /// field — e.g. `omnia@v1`. Must be non-empty after trim.
        #[arg(long)]
        schema: String,
        /// Domain-level characterisation of the project. Required when
        /// the registry already declares another project (RFC-3b
        /// description-missing-multi-repo invariant).
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove an existing project entry from `registry.yaml`
    /// (RFC-9 §2A).
    ///
    /// Loads the registry, removes the named entry, validates the
    /// remaining shape, and persists the result. Warns on stderr (or
    /// in the JSON `warnings` array) when `plan.yaml` exists
    /// and any plan entry references the removed project — the
    /// operator must rewire those entries via `specify change plan amend
    /// --project ...` separately. The warning is non-fatal.
    Remove {
        /// Kebab-case project name to remove.
        name: String,
    },
}

#[derive(Subcommand)]
pub enum MigrateAction {
    /// Move v1 layout artifacts (`registry.yaml`, `plan.yaml`,
    /// `initiative.md`, `contracts/`) from `.specify/` to the repo
    /// root. Idempotent: re-running on an already-migrated project
    /// exits 0 with `nothing to migrate`. Refuses to clobber an
    /// existing destination — if a root-level conflict is present,
    /// inspect manually and resolve before retrying. Refuses inside
    /// a workspace clone (`.specify/workspace/<name>/`).
    V2Layout {
        /// Show what would move without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rename `.specify/changes/` to `.specify/slices/` and rewrite
    /// any in-tree `$CHANGE_DIR` substitutions in vendored skill
    /// markdown to `$SLICE_DIR` (RFC-13 chunk 3.6).
    ///
    /// Idempotent: re-running on an already-migrated project (no
    /// `.specify/changes/` and `.specify/slices/` already in place,
    /// or both directories absent) exits 0 with a "no slices to
    /// migrate" / "already migrated" message. Refuses to run when
    /// any per-loop unit under `.specify/changes/` carries a
    /// non-terminal lifecycle status — the operator must finish or
    /// drop the in-progress slice before migrating
    /// (`slice-migration-blocked-by-in-progress`). Refuses with
    /// `slice-migration-target-exists` when both `.specify/changes/`
    /// and `.specify/slices/` are present (a previous migration was
    /// interrupted or someone hand-edited the tree).
    ///
    /// Single-shot: the migration does not journal its own progress.
    /// If interrupted mid-step, the operator can re-run; the
    /// idempotency guard makes the second run safe.
    SliceLayout {
        /// Show what would change without modifying any file. The
        /// preflight (in-progress detection, target collision check)
        /// still runs and surfaces the same diagnostics it would in a
        /// real run.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rename the umbrella operator brief from `initiative.md` to
    /// `change.md` at the repo root (RFC-13 chunk 3.7).
    ///
    /// Idempotent: re-running on an already-migrated project (only
    /// `change.md` present, or neither file present) exits 0 with a
    /// "nothing to migrate" / "already migrated" message. Refuses
    /// with `change-noun-migration-target-exists` when both
    /// `initiative.md` and `change.md` are present at the repo root
    /// (a previous migration was interrupted or someone hand-edited
    /// the tree); the operator must reconcile manually before
    /// re-running. No on-disk changes to other platform artefacts
    /// (`registry.yaml`, `plan.yaml`, `contracts/` stay put per
    /// RFC-9 §1B).
    ///
    /// Single-shot: this migration does not journal its own progress.
    /// If interrupted mid-step, the operator simply re-runs; the
    /// idempotency guard makes the second run safe.
    ChangeNoun {
        /// Show what would change without modifying any file. The
        /// detection (target collision check) still runs and surfaces
        /// the same diagnostics it would in a real run.
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
    /// Validate a capability.yaml file
    ///
    /// Reads `<capability_dir>/capability.yaml`. Refuses with the
    /// `schema-became-capability` diagnostic when the directory carries
    /// only the pre-RFC-13 `schema.yaml` shape.
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
        /// Outcome classification.
        ///
        /// `success` / `failure` / `deferred` are the original three;
        /// `registry-amendment-required` is RFC-9 §2B and requires
        /// the four `--proposed-*` flags plus `--rationale`.
        #[arg(value_enum)]
        outcome: OutcomeKind,
        /// Short explanation of what happened (shown in plan
        /// status-reason on non-success). Optional only for
        /// `registry-amendment-required` — the CLI synthesises a
        /// canonical `registry-amendment-required: <name>` summary
        /// from `--proposed-name` when omitted.
        #[arg(long)]
        summary: Option<String>,
        /// Optional verbatim detail (stderr, ambiguous-requirement text, etc.)
        #[arg(long)]
        context: Option<String>,
        /// Proposed kebab-case project name. Required (and only
        /// accepted) when `<outcome>` is `registry-amendment-required`.
        #[arg(long)]
        proposed_name: Option<String>,
        /// Proposed clone URL — same shape as `specify registry add --url`.
        /// Required when `<outcome>` is `registry-amendment-required`.
        #[arg(long)]
        proposed_url: Option<String>,
        /// Proposed capability identifier carried by `--proposed-schema`
        /// (e.g. `omnia@v1`). Required
        /// when `<outcome>` is `registry-amendment-required`.
        #[arg(long)]
        proposed_schema: Option<String>,
        /// Optional human-readable description of the proposed
        /// project. Honoured only with `<outcome> = registry-amendment-required`.
        #[arg(long)]
        proposed_description: Option<String>,
        /// Free-form prose explaining why the phase decided this
        /// amendment was required. Required when `<outcome>` is
        /// `registry-amendment-required`.
        #[arg(long)]
        rationale: Option<String>,
    },
    /// Read the stamped `.metadata.yaml.outcome` for a slice
    ///
    /// Symmetric read verb for `outcome set`: emits the current
    /// `outcome` subtree for consumers like `/spec:execute` that
    /// classify a phase return without needing the rest of the
    /// lifecycle-status payload. Exits 0 both when an outcome is
    /// present and when the slice is unstamped (`outcome: null`).
    Show {
        /// Slice name
        name: String,
    },
}

/// CLI-side discriminant for `slice outcome set <outcome>`.
///
/// Mirrors the on-disk [`specify::Outcome`] discriminant strings
/// (kebab-case) but keeps the variants unit-only so clap can derive
/// `ValueEnum`. The dispatcher in `src/commands/slice.rs` reads this
/// alongside the `--proposed-*` / `--rationale` flags and constructs
/// the actual `Outcome` enum value.
///
/// Adding a new outcome requires extending **both** this enum and the
/// `specify::Outcome` enum in `crates/slice/src/lib.rs` (the wire
/// type) — the kebab-case spelling MUST match.
#[derive(Copy, Clone, ValueEnum, PartialEq, Eq, Debug)]
pub enum OutcomeKind {
    /// Phase completed successfully.
    Success,
    /// Phase failed.
    Failure,
    /// Phase deferred (needs human input).
    Deferred,
    /// Phase blocked on a registry amendment (RFC-9 §2B). Requires
    /// the `--proposed-name` / `--proposed-url` / `--proposed-schema`
    /// / `--rationale` flags.
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
