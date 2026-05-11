//! Dispatcher for `specify slice *`.
//!
//! Per-subcommand handlers live in submodules under `slice/`. This file
//! owns the `match action` table and the omnia `artifact_classes`
//! synthesiser shared by `slice merge` and `slice touched-specs`.

use std::path::Path;

use specify_config::ProjectConfig;
use specify_error::Result;
use specify_merge::{ArtifactClass, MergeStrategy};

use crate::cli::{JournalAction, OutcomeAction, SliceAction, SliceMergeAction, SliceTaskAction};
use crate::context::CommandContext;
use crate::output::CliResult;

pub mod cli;
mod journal;
mod lifecycle;
mod list;
mod merge;
mod outcome;
mod task;
mod touched;
mod validate;

pub(super) use list::{StatusEntry, collect_status, list_slice_names, status_entry_to_json};

/// Default omnia [`ArtifactClass`] set: `specs` (3-way merge) and
/// `contracts` (opaque replace). Single source of truth in the
/// binary; future capability manifests should drive this through
/// `specify-capability`.
pub(super) fn artifact_classes(project_root: &Path, slice_dir: &Path) -> Vec<ArtifactClass> {
    vec![
        ArtifactClass {
            name: "specs".to_string(),
            staged_dir: slice_dir.join("specs"),
            baseline_dir: ProjectConfig::specify_dir(project_root).join("specs"),
            strategy: MergeStrategy::ThreeWayMerge,
        },
        ArtifactClass {
            name: "contracts".to_string(),
            staged_dir: slice_dir.join("contracts"),
            baseline_dir: project_root.join("contracts"),
            strategy: MergeStrategy::OpaqueReplace,
        },
    ]
}

pub fn run(ctx: &CommandContext, action: SliceAction) -> Result<CliResult> {
    // Most arms are pure-Success leaf handlers (return `Result<()>`)
    // — only `validate::run` conditionally surfaces a non-success
    // exit, so we lift the rest into `CliResult::Success` here.
    let ok = |()| CliResult::Success;
    match action {
        SliceAction::Create {
            name,
            capability,
            if_exists,
        } => lifecycle::create(ctx, name, capability, if_exists.into()).map(ok),
        SliceAction::List => list::run(ctx).map(ok),
        SliceAction::Status { name } => list::status_one(ctx, name).map(ok),
        SliceAction::Validate { name } => validate::run(ctx, name),
        SliceAction::Merge { action } => match action {
            SliceMergeAction::Run { name } => merge::run(ctx, name).map(ok),
            SliceMergeAction::Preview { name } => merge::preview(ctx, name).map(ok),
            SliceMergeAction::ConflictCheck { name } => merge::conflicts(ctx, name).map(ok),
        },
        SliceAction::Task { action } => match action {
            SliceTaskAction::Progress { name } => task::progress(ctx, name).map(ok),
            SliceTaskAction::Mark { name, task_number } => {
                task::mark(ctx, name, task_number).map(ok)
            }
        },
        SliceAction::Outcome { action } => match action {
            OutcomeAction::Set { name, phase, kind } => {
                outcome::set(ctx, name, phase, kind).map(ok)
            }
            OutcomeAction::Show { name } => outcome::show(ctx, name).map(ok),
        },
        SliceAction::Journal { action } => match action {
            JournalAction::Append {
                name,
                phase,
                kind,
                summary,
                context,
            } => journal::append(ctx, name, phase, kind, summary, context).map(ok),
            JournalAction::Show { name } => journal::show(ctx, name).map(ok),
        },
        SliceAction::Transition { name, target } => {
            lifecycle::transition(ctx, name, target).map(ok)
        }
        SliceAction::TouchedSpecs { name, scan, set } => {
            touched::touched_specs(ctx, name, scan, set).map(ok)
        }
        SliceAction::Overlap { name } => touched::overlap(ctx, name).map(ok),
        SliceAction::Archive { name } => lifecycle::archive(ctx, name).map(ok),
        SliceAction::Drop { name, reason } => lifecycle::drop_slice(ctx, name, reason).map(ok),
    }
}
