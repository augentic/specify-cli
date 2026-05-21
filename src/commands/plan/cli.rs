//! Clap derive surface for `specify plan *` and the nested
//! `plan lock *` verbs. The umbrella `cli.rs` re-exports both action
//! enums.

use clap::{ArgAction, Subcommand};

use crate::cli::{SliceSourceArg, SourceArg};

/// Plan-authoring verbs (`specify plan *`).
#[derive(Subcommand)]
pub enum PlanAction {
    /// Scaffold an empty `plan.yaml` at the repo root. Refuses to
    /// overwrite an existing plan.
    Create {
        /// Kebab-case change name
        name: String,
        /// Named source, repeated: --source `<key>`=`<path-or-url>`.
        /// Recorded in the plan's `sources:` map.
        #[arg(long = "source")]
        sources: Vec<SourceArg>,
    },
    /// Validate plan.yaml (structure + plan/change consistency).
    ///
    /// Includes the four health diagnostics — `cycle-in-depends-on`,
    /// `orphan-source-key`, `stale-workspace-clone`, and
    /// `unreachable-entry` — alongside the base shape rules.
    Validate,
    /// Return the active in-progress entry, or transition the next eligible
    /// `Pending` entry to `InProgress` and return it. `plan next` is the
    /// only writer of per-entry `in-progress` (RFC-25 §CLI surface).
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
        /// Per-slice source binding (repeatable). Wire form is
        /// `<key>=<candidate-id>`; bare `<key>` is accepted as
        /// shorthand for `{ key: <key>, candidate: <slice.name> }`
        /// per RFC-25 §`Slice.sources`.
        #[arg(long = "sources", action = ArgAction::Append)]
        sources: Vec<SliceSourceArg>,
        /// Free-text scoping hint for the define step
        #[arg(long)]
        description: Option<String>,
        /// Target registry project name
        #[arg(long)]
        project: Option<String>,
        /// Plan-entry target-adapter identifier for project-less entries (e.g. `contracts@v1`)
        #[arg(long)]
        target: Option<String>,
        /// Baseline paths relevant to this change, relative to `.specify/` (repeatable)
        #[arg(long)]
        context: Vec<String>,
    },
    /// Edit non-status fields on an existing plan entry.
    ///
    /// Three orthogonal flag families operate on `sources`:
    ///
    /// - `--sources <binding>` (with `num_args = 0..`) replaces the
    ///   slice's `sources` array wholesale.
    /// - `--add-source <binding>` (repeatable) adds a single binding.
    /// - `--remove-source <key>` (repeatable) removes a binding by
    ///   key; fails with `plan-binding-not-found` when no binding
    ///   matches.
    ///
    /// `--add-source` and `--remove-source` apply after `--sources`,
    /// so wholesale replacement plus targeted edits can be combined
    /// in a single invocation when needed.
    Amend {
        /// Kebab-case change name
        name: String,
        /// Replace depends-on. Pass `--depends-on` (with no value) to clear the
        /// field; omit the flag to leave it unchanged. Repeat or comma-separate
        /// to supply multiple values.
        #[arg(long = "depends-on", num_args = 0.., value_delimiter = ',')]
        depends_on: Option<Vec<String>>,
        /// Replace per-slice source bindings wholesale. Each value
        /// is `<key>=<candidate-id>` (or bare `<key>` shorthand).
        /// Pass `--sources` (no value) to clear; omit to leave
        /// unchanged.
        #[arg(long = "sources", num_args = 0.., value_delimiter = ',')]
        sources: Option<Vec<SliceSourceArg>>,
        /// Add a single per-slice source binding (repeatable). Each
        /// value is `<key>=<candidate-id>` or the bare `<key>`
        /// shorthand per RFC-25 §`Slice.sources`.
        #[arg(long = "add-source", action = ArgAction::Append)]
        add_source: Vec<SliceSourceArg>,
        /// Remove a per-slice source binding by key (repeatable).
        /// Fails with `plan-binding-not-found` when no such binding
        /// exists on the slice.
        #[arg(long = "remove-source", action = ArgAction::Append)]
        remove_source: Vec<String>,
        /// Set the slice's `divergence` field (RFC-25 §Plan-time
        /// fusion). Only `accepted` and `rejected` are accepted on
        /// the wire; `none` (absent) is the implicit default and
        /// `likely` is reserved for the `propose` sub-step.
        #[arg(long = "divergence")]
        divergence: Option<String>,
        /// Replace description. Pass `--description ""` to clear; omit the flag
        /// to leave it unchanged.
        #[arg(long)]
        description: Option<String>,
        /// Replace project. Pass `--project ""` to clear; omit the flag to leave it unchanged.
        #[arg(long)]
        project: Option<String>,
        /// Replace the plan-entry target-adapter identifier. Pass `--target ""` to clear;
        /// omit the flag to leave it unchanged.
        #[arg(long)]
        target: Option<String>,
        /// Replace context paths. Pass `--context` (with no value) to clear; omit the
        /// flag to leave it unchanged.
        #[arg(long, num_args = 0.., value_delimiter = ',')]
        context: Option<Vec<String>>,
    },
    /// Apply a validated status transition.
    ///
    /// Two transition shapes share this verb (RFC-25 §CLI surface):
    ///
    /// - **Plan-level Gate 1 stamp** — `<name>` is the plan name and
    ///   `<target>` is `reviewed`. Operator-only — `/spec:plan` MUST
    ///   NOT call this verb; skill bodies stop at `pending` and print
    ///   the literal `specify plan transition <name> reviewed`
    ///   command in their closing hint for the operator to run.
    /// - **Per-entry close** — `<name>` is a plan-entry name and
    ///   `<target>` is `done`. The `/spec:merge` skill is the
    ///   canonical caller.
    ///
    /// Per-entry `pending` is written only by `plan add` / `plan amend`;
    /// per-entry `in-progress` is written only by `plan next`. v1 has
    /// no per-entry `blocked`, `failed`, or `skipped` state — build
    /// failures and merge conflicts leave the active entry `in-progress`.
    Transition {
        /// Plan name (for plan-level `reviewed`) or kebab-case entry
        /// name (for per-entry `done`).
        name: String,
        /// Transition target — `reviewed` (plan-level) or `done`
        /// (per-entry).
        target: String,
    },
    /// Archive the current plan to `.specify/archive/plans/<name>-<YYYYMMDD>.yaml`
    Archive {
        /// Archive even when the plan has pending or in-progress entries.
        /// Without --force, these non-terminal statuses block the archive.
        #[arg(long)]
        force: bool,
    },
    /// Driver-lock primitives — `.specify/plan.lock` PID stamp used by
    /// `/spec:execute` to serialise concurrent drivers.
    Lock {
        #[command(subcommand)]
        action: LockAction,
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
