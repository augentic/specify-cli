//! Dispatcher for `specify slice *`. Owns the `match action` table and
//! the omnia `artifact_classes` synthesiser shared by `slice merge` and
//! `slice touched-specs`.

use std::path::Path;

use specify_domain::config::Layout;
use specify_domain::merge::{ArtifactClass, MergeStrategy};
use specify_domain::slice::LifecycleStatus;
use specify_error::{Error, Result};

pub mod cli;
mod lifecycle;
mod merge;
mod task;
mod touched;
mod validate;

use cli::{SliceAction, SliceMergeAction, SliceTaskAction};

use crate::context::Ctx;

/// Default omnia [`ArtifactClass`] set: `specs` (3-way merge) and
/// `contracts` (opaque replace). Single source of truth in the
/// binary; future adapter manifests should drive this through
/// `specify-adapter`.
fn artifact_classes(project_root: &Path, slice_dir: &Path) -> Vec<ArtifactClass> {
    vec![
        ArtifactClass {
            name: "specs".to_string(),
            staged_dir: slice_dir.join("specs"),
            baseline_dir: Layout::new(project_root).specify_dir().join("specs"),
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

pub fn run(ctx: &Ctx, action: SliceAction) -> Result<()> {
    match action {
        SliceAction::Create {
            name,
            target,
            if_exists,
        } => lifecycle::create(ctx, &name, target, if_exists),
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
        SliceAction::Transition { name, target } => {
            if matches!(target, LifecycleStatus::Merged) {
                return Err(Error::Argument {
                    flag: "<target>",
                    detail: "use `specify slice merge run` to reach `merged`".to_string(),
                });
            }
            lifecycle::transition(ctx, name, target)
        }
        SliceAction::TouchedSpecs { name, scan, set } => touched::specs(ctx, name, scan, &set),
        SliceAction::Overlap { name } => touched::overlap(ctx, name),
        SliceAction::Drop { name, reason } => {
            lifecycle::discard_slice(ctx, name, reason.as_deref())
        }
    }
}
