//! Clap derive surface for `specify change plan *` and the nested
//! `plan lock *` verbs.
//!
//! Lifted out of `src/cli.rs`; `cli.rs` re-exports both action enums so
//! the umbrella `Cli` / `Commands` / `ChangeAction` derives resolve at
//! expansion time.

use clap::{ArgAction, Subcommand};
use specify_change::Status;

use crate::cli::parse_source_kv;

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
