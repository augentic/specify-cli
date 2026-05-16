//! Clap derive surface for `specify plan *` and the nested
//! `plan lock *` verbs. The umbrella `cli.rs` re-exports both action
//! enums.

use clap::{ArgAction, Subcommand};
use specify_domain::change::Status;

use crate::cli::SourceArg;

/// Plan-authoring verbs (`specify plan *`).
///
/// `specify change draft` scaffolds `change.md` and `plan.yaml`
/// together; the same scaffolding is also reachable plan-only via
/// [`PlanAction::Create`] when no operator brief is wanted.
#[derive(Subcommand)]
pub enum PlanAction {
    /// Scaffold an empty `plan.yaml` at the repo root. Refuses to
    /// overwrite an existing plan. Shares its scaffold helper with
    /// `specify change draft`, which also writes `change.md`.
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
    /// `unreachable-entry` — alongside the base shape rules. The first
    /// triage step when `/change:execute loop` reports `stuck`.
    Validate,
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
        /// Target registry project name
        #[arg(long)]
        project: Option<String>,
        /// Plan-entry `capability` target for project-less entries (e.g. `contracts@v1`)
        #[arg(long)]
        capability: Option<String>,
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
        /// Replace the plan-entry `capability` target. Pass `--capability ""` to clear;
        /// omit the flag to leave it unchanged.
        #[arg(long)]
        capability: Option<String>,
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
    /// Archive the current plan to `.specify/archive/plans/<name>-<YYYYMMDD>.yaml`
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
