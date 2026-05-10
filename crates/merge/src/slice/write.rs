//! Commit side of the slice-merge engine: atomic baseline writes for
//! 3-way classes, opaque-replace whole-file copy, and the
//! summary-string builder stamped into `.metadata.yaml.outcome`.
//!
//! Every helper here mutates the filesystem. They are only invoked
//! from [`super::merge_slice`] *after* the in-memory plan from
//! [`super::read::plan_three_way`] has merged and validated cleanly,
//! so partial writes here imply a real disk failure rather than a
//! recoverable conflict.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use specify_error::Error;

use super::MergePreviewEntry;
use crate::artifact_class::{ArtifactClass, MergeStrategy};

/// Write each merged baseline produced by [`super::read::plan_three_way`]
/// to its target path, creating parent directories on demand. Caller
/// guarantees every entry has already validated.
pub(super) fn write_three_way_baselines(merged: &[MergePreviewEntry]) -> Result<(), Error> {
    for entry in merged {
        if let Some(parent) = entry.baseline_path.parent() {
            fs::create_dir_all(parent).map_err(|err| Error::Diag {
                code: "merge-mkdir-failed",
                detail: format!("failed to create {}: {err}", parent.display()),
            })?;
        }
        fs::write(&entry.baseline_path, &entry.result.output).map_err(|err| Error::Diag {
            code: "merge-write-baseline-failed",
            detail: format!("failed to write baseline {}: {err}", entry.baseline_path.display()),
        })?;
    }
    Ok(())
}

/// For every [`MergeStrategy::OpaqueReplace`] class, recursively copy
/// `class.staged_dir` over `class.baseline_dir`, returning the per-class
/// count of files copied. Empty staged directories are skipped without
/// recording an entry, so the resulting map only carries classes that
/// actually contributed work.
pub(super) fn commit_opaque(classes: &[ArtifactClass]) -> Result<BTreeMap<String, usize>, Error> {
    let mut opaque_counts: BTreeMap<String, usize> = BTreeMap::new();
    for class in classes.iter().filter(|c| matches!(c.strategy, MergeStrategy::OpaqueReplace)) {
        if !class.staged_dir.is_dir() {
            continue;
        }
        let copied = copy_opaque(&class.staged_dir, &class.baseline_dir)?;
        if !copied.is_empty() {
            opaque_counts.insert(class.name.clone(), copied.len());
        }
    }
    Ok(opaque_counts)
}

/// Recursively copy all files from `src` into `dest`, preserving the
/// relative directory structure. Existing files at the same relative
/// path are replaced (opaque whole-file replacement, not delta-merge).
/// Returns the list of relative paths that were copied.
fn copy_opaque(src: &Path, dest: &Path) -> Result<Vec<String>, Error> {
    let mut copied = Vec::new();
    copy_opaque_recursive(src, dest, src, &mut copied)?;
    Ok(copied)
}

fn copy_opaque_recursive(
    base: &Path, dest_base: &Path, current: &Path, copied: &mut Vec<String>,
) -> Result<(), Error> {
    for entry in fs::read_dir(current).map_err(|err| Error::Diag {
        code: "merge-readdir-failed",
        detail: format!("failed to read {}: {err}", current.display()),
    })? {
        let entry = entry.map_err(|err| Error::Diag {
            code: "merge-dir-entry-failed",
            detail: format!("dir entry error: {err}"),
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(base).map_err(|err| Error::Diag {
            code: "merge-path-prefix-failed",
            detail: format!("path prefix error: {err}"),
        })?;
        let dest_path = dest_base.join(relative);

        if path.is_dir() {
            fs::create_dir_all(&dest_path).map_err(|err| Error::Diag {
                code: "merge-mkdir-failed",
                detail: format!("failed to create {}: {err}", dest_path.display()),
            })?;
            copy_opaque_recursive(base, dest_base, &path, copied)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|err| Error::Diag {
                    code: "merge-mkdir-failed",
                    detail: format!("failed to create {}: {err}", parent.display()),
                })?;
            }
            fs::copy(&path, &dest_path).map_err(|err| Error::Diag {
                code: "merge-copy-failed",
                detail: format!(
                    "failed to copy {} to {}: {err}",
                    path.display(),
                    dest_path.display()
                ),
            })?;
            copied.push(relative.to_string_lossy().to_string());
        }
    }
    Ok(())
}

/// Build the operator-facing summary stamped onto the merge phase
/// outcome. Format: `Merged <count> <class>[, <count> <class>]* into
/// baseline`. Empty merges (no work) round-trip as
/// `Merged 0 entries into baseline` so the field is never blank.
pub(super) fn build_merge_summary(
    three_way: &[MergePreviewEntry], opaque_counts: &BTreeMap<String, usize>,
) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in three_way {
        *counts.entry(entry.class_name.clone()).or_insert(0) += 1;
    }
    for (name, count) in opaque_counts {
        *counts.entry(name.clone()).or_insert(0) += count;
    }
    if counts.is_empty() {
        return "Merged 0 entries into baseline".to_string();
    }
    let parts: Vec<String> =
        counts.iter().map(|(class, count)| format!("{count} {class}")).collect();
    format!("Merged {} into baseline", parts.join(", "))
}
