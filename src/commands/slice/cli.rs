//! Clap derive surface for `specify slice *` and its nested verbs.
//! The umbrella `cli.rs` re-exports the action enums.

use clap::Subcommand;
use serde::Deserialize;
use specify_domain::capability::Phase;
use specify_domain::slice::{CreateIfExists, EntryKind, LifecycleStatus};

#[derive(Subcommand)]
pub enum SliceAction {
    /// Create a new slice directory with an initial `.metadata.yaml`
    Create {
        /// Kebab-case slice name
        name: String,
        /// Capability identifier; defaults to the value in `.specify/project.yaml`
        #[arg(long)]
        capability: Option<String>,
        /// Behaviour when `<slices_dir>/<name>/` already exists
        #[arg(long, value_enum, default_value = "fail")]
        if_exists: CreateIfExists,
    },
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
    /// Transition a slice to a new lifecycle status. Note: `merged` is
    /// not a valid target — the only legal writer of `Merged` is
    /// `specify slice merge run`, which performs the spec merge,
    /// status transition, and archive move atomically.
    Transition {
        /// Slice name
        name: String,
        /// Target status (`defined`, `building`, `complete`, `dropped`, or `defining`).
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
        /// Replace `touched_specs` with the listed capabilities (each `<name>:new|modified`)
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

/// Phase-outcome subcommands grouped under `slice outcome`.
#[derive(Subcommand)]
pub enum OutcomeAction {
    /// Record the outcome of a phase (define|build|merge) on `.metadata.yaml`.
    /// The outcome kind is itself a subcommand so each variant carries
    /// only its own flags.
    Set {
        /// Slice name
        name: String,
        /// Phase this outcome applies to
        #[arg(value_enum)]
        phase: Phase,
        #[command(subcommand)]
        kind: OutcomeKindAction,
    },
    /// Read the stamped `.metadata.yaml.outcome` for a slice. Exits 0
    /// whether or not an outcome has been stamped.
    Show {
        /// Slice name
        name: String,
    },
}

/// Outcome-kind subcommands under `slice outcome set`. Each variant
/// owns the flags that are valid for it; clap rejects everything else.
#[derive(Subcommand)]
pub enum OutcomeKindAction {
    /// Phase completed successfully.
    Success {
        /// Short explanation of what happened.
        #[arg(long)]
        summary: String,
        /// Optional verbatim detail (stderr, log excerpt, ...).
        #[arg(long)]
        context: Option<String>,
    },
    /// Phase failed.
    Failure {
        /// Short explanation of what happened.
        #[arg(long)]
        summary: String,
        /// Optional verbatim detail (stderr, log excerpt, ...).
        #[arg(long)]
        context: Option<String>,
    },
    /// Phase deferred (needs human input).
    Deferred {
        /// Short explanation of what happened.
        #[arg(long)]
        summary: String,
        /// Optional verbatim detail (stderr, ambiguous-requirement text, ...).
        #[arg(long)]
        context: Option<String>,
    },
    /// Phase blocked on a registry amendment.
    RegistryAmendmentRequired {
        /// Short explanation; defaults to `registry-amendment-required: <proposed-name>`.
        #[arg(long)]
        summary: Option<String>,
        /// Optional verbatim detail.
        #[arg(long)]
        context: Option<String>,
        /// Structured proposal payload as a single JSON object.
        ///
        /// Required keys: `proposed-name`, `proposed-url`,
        /// `proposed-capability`, `rationale`. Optional:
        /// `proposed-description`. Skill drivers build the JSON object;
        /// humans never type it. Parsed via `serde_json::from_str` —
        /// malformed JSON or missing required keys exit `2` with a
        /// kebab-case `proposal-invalid` diagnostic.
        #[arg(long, value_parser = parse_proposal)]
        proposal: RegistryAmendmentProposal,
    },
}

/// Structured payload supplied via `--proposal '<json>'`.
///
/// Mirrors the on-disk `outcome.outcome.registry-amendment-required.*`
/// shape one-for-one (kebab-case keys); `lower_kind` lifts it into
/// `OutcomeKind::RegistryAmendmentRequired` without further validation.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RegistryAmendmentProposal {
    /// Proposed kebab-case project name.
    pub(crate) proposed_name: String,
    /// Proposed clone URL.
    pub(crate) proposed_url: String,
    /// Proposed capability identifier (e.g. `omnia@v1`).
    pub(crate) proposed_capability: String,
    /// Optional human-readable description of the proposed project.
    #[serde(default)]
    pub(crate) proposed_description: Option<String>,
    /// Rationale prose.
    pub(crate) rationale: String,
}

/// `value_parser` for `--proposal <json>`. Maps `serde_json` errors
/// onto a clap-friendly `String` so the standard parse-error exit path
/// (exit `2`, plain stderr) handles them — matching every other typed
/// `value_parser` in the surface.
fn parse_proposal(raw: &str) -> Result<RegistryAmendmentProposal, String> {
    serde_json::from_str(raw).map_err(|err| format!("--proposal: {err}"))
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
