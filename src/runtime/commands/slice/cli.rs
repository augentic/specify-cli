//! Clap derive surface for `specrun slice *` and its nested verbs.
//! The umbrella `cli.rs` re-exports the action enums.

use clap::Subcommand;
use specify_workflow::slice::{CreateIfExists, LifecycleStatus};

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
    /// Transition a slice to a new lifecycle status. Note: `merged` is
    /// not a valid target — the only legal writer of `Merged` is
    /// `specrun slice merge run`, which performs the spec merge,
    /// status transition, and archive move atomically.
    Transition {
        /// Slice name
        name: String,
        /// Target status (`refining`, `refined`, `built`, or `dropped`).
        /// `merged` is reserved for `specrun slice merge run` and is
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
