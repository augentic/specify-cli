//! Dispatcher for `specify slice *`. Owns the `match action` table and
//! the omnia `artifact_classes` synthesiser shared by `slice merge` and
//! `slice touched-specs`.

use std::path::Path;

use specify_domain::config::LayoutExt;
use specify_domain::merge::{ArtifactClass, MergeStrategy};
use specify_error::Result;

use crate::cli::{JournalAction, OutcomeAction, SliceAction, SliceMergeAction, SliceTaskAction};
use crate::context::Ctx;

pub(crate) mod cli;
mod journal;
mod lifecycle;
mod list;
mod merge;
mod outcome;
mod task;
mod touched;
mod validate;

pub(super) use list::{StatusEntry, collect_status, list_slice_names};

/// Default omnia [`ArtifactClass`] set: `specs` (3-way merge) and
/// `contracts` (opaque replace). Single source of truth in the
/// binary; future capability manifests should drive this through
/// `specify-capability`.
pub(super) fn artifact_classes(project_root: &Path, slice_dir: &Path) -> Vec<ArtifactClass> {
    vec![
        ArtifactClass {
            name: "specs".to_string(),
            staged_dir: slice_dir.join("specs"),
            baseline_dir: project_root.layout().specify_dir().join("specs"),
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

pub(crate) fn run(ctx: &Ctx, action: SliceAction) -> Result<()> {
    match action {
        SliceAction::Create {
            name,
            capability,
            if_exists,
        } => lifecycle::create(ctx, &name, capability, if_exists.into()),
        SliceAction::List => list::run(ctx),
        SliceAction::Status { name } => list::status_one(ctx, &name),
        SliceAction::Validate { name } => validate::run(ctx, &name),
        SliceAction::Merge { action } => match action {
            SliceMergeAction::Run { name } => merge::run(ctx, &name),
            SliceMergeAction::Preview { name } => merge::preview(ctx, &name),
            SliceMergeAction::ConflictCheck { name } => merge::conflicts(ctx, &name),
        },
        SliceAction::Task { action } => match action {
            SliceTaskAction::Progress { name } => task::progress(ctx, &name),
            SliceTaskAction::Mark { name, task_number } => task::mark(ctx, &name, task_number),
        },
        SliceAction::Outcome { action } => match action {
            OutcomeAction::Set { name, phase, kind } => outcome::set(ctx, name, phase, kind),
            OutcomeAction::Show { name } => outcome::show(ctx, name),
        },
        SliceAction::Journal { action } => match action {
            JournalAction::Append {
                name,
                phase,
                kind,
                summary,
                context,
            } => journal::append(ctx, name, phase, kind, summary, context),
            JournalAction::Show { name } => journal::show(ctx, name),
        },
        SliceAction::Transition { name, target } => lifecycle::transition(ctx, name, target),
        SliceAction::TouchedSpecs { name, scan, set } => touched::specs(ctx, name, scan, &set),
        SliceAction::Overlap { name } => touched::overlap(ctx, name),
        SliceAction::Archive { name } => lifecycle::archive(ctx, name),
        SliceAction::Drop { name, reason } => {
            lifecycle::discard_slice(ctx, name, reason.as_deref())
        }
    }
}
