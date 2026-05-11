//! `transition` verb: validate a lifecycle edge and stamp the matching
//! `*_at` timestamp.

use std::path::Path;

use chrono::{DateTime, Utc};
use specify_error::Error;

use crate::slice::{LifecycleStatus, SliceMetadata};

/// Transition a slice to `target` status and write the matching timestamp.
///
/// The transition is validated by
/// [`LifecycleStatus::transition`](crate::slice::LifecycleStatus::transition) —
/// illegal edges return `Error::Lifecycle` without touching disk. On
/// success the metadata's `status` is updated, the appropriate
/// `*_at` timestamp is filled in (idempotent: an existing non-`None`
/// timestamp is preserved), and `.metadata.yaml` is rewritten atomically.
///
/// Returns the updated `SliceMetadata`.
///
/// # Errors
///
/// `Error::Lifecycle` for an illegal edge; otherwise propagates load /
/// save failures from `SliceMetadata`.
pub fn transition(
    slice_dir: &Path, target: LifecycleStatus, now: DateTime<Utc>,
) -> Result<SliceMetadata, Error> {
    let mut metadata = SliceMetadata::load(slice_dir)?;
    metadata.status = metadata.status.transition(target)?;
    let stamp = now;
    match target {
        LifecycleStatus::Defining => {
            if metadata.created_at.is_none() {
                metadata.created_at = Some(stamp);
            }
        }
        LifecycleStatus::Defined => {
            if metadata.defined_at.is_none() {
                metadata.defined_at = Some(stamp);
            }
        }
        LifecycleStatus::Building => {
            if metadata.build_started_at.is_none() {
                metadata.build_started_at = Some(stamp);
            }
        }
        LifecycleStatus::Complete => {
            if metadata.completed_at.is_none() {
                metadata.completed_at = Some(stamp);
            }
        }
        LifecycleStatus::Merged => {
            if metadata.merged_at.is_none() {
                metadata.merged_at = Some(stamp);
            }
        }
        LifecycleStatus::Dropped => {
            if metadata.dropped_at.is_none() {
                metadata.dropped_at = Some(stamp);
            }
        }
    }
    metadata.save(slice_dir)?;
    Ok(metadata)
}
