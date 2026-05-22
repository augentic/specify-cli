//! Clap derive surface for `specify slice *` and its nested verbs.
//! The umbrella `cli.rs` re-exports the action enums.

use clap::Subcommand;
use specify_domain::slice::{CreateIfExists, LifecycleStatus};

#[derive(Subcommand)]
pub enum SliceAction {
    /// Create a new slice directory with an initial `.metadata.yaml`
    Create {
        /// Kebab-case slice name
        name: String,
        /// Target-adapter identifier; defaults to the value in `.specify/project.yaml`
        #[arg(long)]
        target: Option<String>,
        /// Behaviour when `<slices_dir>/<name>/` already exists
        #[arg(long, value_enum, default_value = "fail")]
        if_exists: CreateIfExists,
    },
    /// Show the status of one slice
    Status {
        /// Slice name (under `.specify/slices/`)
        name: String,
    },
    /// Validate a slice's artifacts against adapter validation rules
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
    /// Operation-outcome bookkeeping on `.metadata.yaml`
    Outcome {
        #[command(subcommand)]
        action: OutcomeAction,
    },
    /// Reconciliation index (`fusion.yaml`) inspection. Per RFC-27
    /// §D4 the file lists every `REQ-*` id in `spec.md` and the
    /// contributing `(source-key, claim-id)` pairs plus the
    /// authority outcome. Writes belong to `/spec:refine`; this
    /// verb owns inspection only.
    Fusion {
        #[command(subcommand)]
        action: SliceFusionAction,
    },
    /// Transition a slice to a new lifecycle status. Note: `merged` is
    /// not a valid target — the only legal writer of `Merged` is
    /// `specify slice merge run`, which performs the spec merge,
    /// status transition, and archive move atomically.
    Transition {
        /// Slice name
        name: String,
        /// Target status (`refining`, `refined`, `built`, or `dropped`).
        /// `merged` is reserved for `specify slice merge run` and is
        /// rejected with exit 2 if passed here.
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
        /// Replace `touched_specs` with the listed adapters (each `<name>:new|modified`)
        #[arg(long, value_delimiter = ',')]
        set: Vec<String>,
    },
    /// Report overlapping `touched_specs` with other active slices
    Overlap {
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

/// Operation-outcome subcommands grouped under `slice outcome`.
#[derive(Subcommand)]
pub enum OutcomeAction {
    /// Read the stamped `.metadata.yaml.outcome` for a slice. Exits 0
    /// whether or not an outcome has been stamped.
    Show {
        /// Slice name
        name: String,
    },
}

/// Reconciliation-index (`fusion.yaml`) subcommands grouped under
/// `slice fusion`. Only `show` lands in 2.6; writes belong to the
/// `/spec:refine` skill body (Change 3.2).
#[derive(Subcommand)]
pub enum SliceFusionAction {
    /// Render the slice's `fusion.yaml` after schema validation.
    /// `--format json` re-emits the parsed shape verbatim; `--format
    /// text` (default) renders one requirement per section with the
    /// inline `value` payload truncated for terminal display.
    Show {
        /// Slice name (under `.specify/slices/`)
        name: String,
    },
}
