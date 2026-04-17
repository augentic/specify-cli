//! Transactional multi-spec merge + archive (`merge_change`).
//!
//! Everything is computed in memory first. We only touch the filesystem
//! after every delta has merged cleanly *and* every merged baseline has
//! passed [`crate::validate_baseline`]. On success we:
//!
//!   1. Write each merged baseline under `specs_dir`.
//!   2. Flip `.metadata.yaml.status` from `Complete` to `Merged`.
//!   3. Move the change directory under `archive_dir` as
//!      `YYYY-MM-DD-<change-name>/`.
//!
//! Any failure before step 1 returns `Err` with the filesystem untouched.

use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use specify_change::{ChangeMetadata, LifecycleStatus};
use specify_error::Error;
use specify_schema::{Phase, PipelineView};

use crate::merge::{MergeResult, merge};
use crate::validate::validate_baseline;

/// Atomic multi-spec merge plus archive.
///
/// See [`crate`] for the step-by-step transactional contract. `change_dir`
/// is expected to live at `<project>/.specify/changes/<name>/`, so
/// `PipelineView` resolution walks up three levels to find the project
/// root — this lets the caller keep the project root implicit. If the
/// layout differs the function returns `Error::Merge` before touching
/// disk.
pub fn merge_change(
    change_dir: &Path,
    specs_dir: &Path,
    archive_dir: &Path,
) -> Result<Vec<(String, MergeResult)>, Error> {
    // --- 1. Load change metadata and gate on `Complete` ---------------------

    let mut metadata = ChangeMetadata::load(change_dir)?;
    if metadata.status != LifecycleStatus::Complete {
        return Err(Error::Lifecycle {
            expected: "Complete".to_string(),
            found: format!("{:?}", metadata.status),
        });
    }

    // --- 2. Resolve the pipeline view ---------------------------------------
    //
    // Convention: `<project>/.specify/changes/<name>/`. Three `.parent()`
    // hops land us at `<project>`. If the caller points this at some
    // other directory shape, abort cleanly.
    let project_dir = change_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            Error::Merge(format!(
                "cannot resolve project root from change dir {}",
                change_dir.display()
            ))
        })?;

    let pipeline_view = PipelineView::load(&metadata.schema, project_dir).map_err(|err| {
        Error::Merge(format!(
            "failed to load pipeline view for schema `{}`: {err}",
            metadata.schema
        ))
    })?;

    // --- 3. Discover delta specs via every `generates` brief under `merge` --

    let mut delta_specs: Vec<DeltaSpecRef> = Vec::new();
    for brief in pipeline_view.phase(Phase::Merge) {
        let Some(glob_pattern) = brief.frontmatter.generates.as_deref() else {
            continue;
        };
        let full_glob = change_dir.join(glob_pattern);
        let pattern_str = full_glob
            .to_str()
            .ok_or_else(|| Error::Merge(format!("non-UTF8 glob path: {}", full_glob.display())))?;

        let entries = glob::glob(pattern_str)
            .map_err(|err| Error::Merge(format!("invalid glob `{pattern_str}`: {err}")))?;
        for entry in entries {
            let delta_path =
                entry.map_err(|err| Error::Merge(format!("glob traversal failure: {err}")))?;
            if !delta_path.is_file() {
                continue;
            }
            let spec_name = derive_spec_name(change_dir, &delta_path);
            let baseline_path = specs_dir.join(&spec_name).join("spec.md");
            delta_specs.push(DeltaSpecRef {
                spec_name,
                delta_path,
                baseline_path,
            });
        }
    }

    // Sort + dedupe by delta_path to keep behaviour deterministic across
    // overlapping globs.
    delta_specs.sort_by(|a, b| a.delta_path.cmp(&b.delta_path));
    delta_specs.dedup_by(|a, b| a.delta_path == b.delta_path);

    // --- 4. In-memory merge + coherence -------------------------------------

    struct MergedEntry {
        spec_name: String,
        baseline_path: PathBuf,
        result: MergeResult,
    }

    let mut merged: Vec<MergedEntry> = Vec::with_capacity(delta_specs.len());
    let mut aborts: Vec<String> = Vec::new();

    for spec in delta_specs {
        let delta_text = std::fs::read_to_string(&spec.delta_path).map_err(|err| {
            Error::Merge(format!(
                "failed to read delta {}: {err}",
                spec.delta_path.display()
            ))
        })?;

        let baseline_text = if spec.baseline_path.is_file() {
            Some(std::fs::read_to_string(&spec.baseline_path).map_err(|err| {
                Error::Merge(format!(
                    "failed to read baseline {}: {err}",
                    spec.baseline_path.display()
                ))
            })?)
        } else {
            None
        };

        let result = match merge(baseline_text.as_deref(), &delta_text) {
            Ok(r) => r,
            Err(Error::Merge(msg)) => {
                aborts.push(format!("{}: {msg}", spec.spec_name));
                continue;
            }
            Err(other) => return Err(other),
        };

        for vr in validate_baseline(&result.output, None) {
            if let specify_schema::ValidationResult::Fail { detail, .. } = vr {
                aborts.push(format!("{}: {detail}", spec.spec_name));
            }
        }

        merged.push(MergedEntry {
            spec_name: spec.spec_name,
            baseline_path: spec.baseline_path,
            result,
        });
    }

    if !aborts.is_empty() {
        return Err(Error::Merge(aborts.join("\n")));
    }

    // --- 5. Commit: write baselines -----------------------------------------

    for entry in &merged {
        if let Some(parent) = entry.baseline_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                Error::Merge(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        std::fs::write(&entry.baseline_path, &entry.result.output).map_err(|err| {
            Error::Merge(format!(
                "failed to write baseline {}: {err}",
                entry.baseline_path.display()
            ))
        })?;
    }

    // --- 6. Metadata flip + archive move -----------------------------------

    metadata.status = metadata.status.transition(LifecycleStatus::Merged)?;
    if metadata.completed_at.is_none() {
        metadata.completed_at = Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
    }
    metadata.save(change_dir)?;

    let change_name = change_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            Error::Merge(format!(
                "change dir {} has no basename",
                change_dir.display()
            ))
        })?;
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let archive_target = archive_dir.join(format!("{date}-{change_name}"));
    std::fs::create_dir_all(archive_dir).map_err(|err| {
        Error::Merge(format!(
            "failed to prepare archive dir {}: {err}",
            archive_dir.display()
        ))
    })?;
    move_dir_atomic(change_dir, &archive_target)?;

    // --- 7. Return merged results sorted by spec name -----------------------

    let mut output: Vec<(String, MergeResult)> = merged
        .into_iter()
        .map(|e| (e.spec_name, e.result))
        .collect();
    output.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(output)
}

struct DeltaSpecRef {
    spec_name: String,
    delta_path: PathBuf,
    baseline_path: PathBuf,
}

/// Derive the logical spec name from a discovered delta path.
///
/// For the expected shape `<change_dir>/specs/<name>/spec.md` we return
/// `<name>`. For anything weirder we fall back to the file stem so the
/// caller still gets something deterministic to key on.
fn derive_spec_name(change_dir: &Path, delta_path: &Path) -> String {
    let rel = delta_path.strip_prefix(change_dir).unwrap_or(delta_path);
    let components: Vec<&std::ffi::OsStr> = rel.iter().collect();
    if components.len() >= 3
        && components[0] == std::ffi::OsStr::new("specs")
        && components[components.len() - 1] == std::ffi::OsStr::new("spec.md")
        && let Some(name) = components[components.len() - 2].to_str()
    {
        return name.to_string();
    }
    delta_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("spec")
        .to_string()
}

/// Move `src` to `dst`. Uses `rename` first, then falls back to
/// copy-then-remove when the rename fails with `EXDEV` (cross-device) —
/// `archive/` can live on a different mount from the working tree.
fn move_dir_atomic(src: &Path, dst: &Path) -> Result<(), Error> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc_exdev()) => {
            copy_dir_recursive(src, dst)?;
            std::fs::remove_dir_all(src).map_err(|err| {
                Error::Merge(format!(
                    "failed to remove source {} after cross-device copy: {err}",
                    src.display()
                ))
            })?;
            Ok(())
        }
        Err(err) => Err(Error::Merge(format!(
            "failed to move {} -> {}: {err}",
            src.display(),
            dst.display()
        ))),
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            symlink(&link_target, &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    // Directory vs file symlink choice on Windows — target metadata gives
    // us the hint; fall back to file-symlink if it can't be read.
    match std::fs::metadata(original) {
        Ok(meta) if meta.is_dir() => std::os::windows::fs::symlink_dir(original, link),
        _ => std::os::windows::fs::symlink_file(original, link),
    }
}

#[cfg(not(any(unix, windows)))]
fn symlink(_original: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlinks unsupported on this platform",
    ))
}

/// EXDEV is "cross-device link" — the exact errno varies by libc but is
/// stable across Linux / macOS / BSD as `18`. Kept as a tiny helper so the
/// call-site above stays readable.
fn libc_exdev() -> i32 {
    18
}
