//! Stage-and-install of a freshly downloaded tool into the global cache.
//!
//! The "fetch" name covers the install step the resolver hands a staged
//! directory to; the actual byte download (`https:` or `file:`) lives in
//! `crate::resolver`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use super::{MAX_TEMP_ATTEMPTS, sorted_dir_entries, unique_sibling_path};
use crate::error::ToolError;

/// Install a staged cache directory into `dest`.
///
/// The staged tree is first copied into a sibling temporary directory. The
/// final switch into place uses `rename`. When replacing an existing cache
/// version, the old directory is first renamed to a sibling backup, then the
/// new complete directory is renamed into place. A crash during replacement
/// can leave the destination absent plus a backup, but never a partially
/// copied destination.
///
/// # Errors
///
/// Returns `ToolError::CacheIo` when `staged` is not a directory, an entry
/// inside it is neither a file nor a directory, the parent of `dest` cannot
/// be created, or any individual file copy fails. Returns
/// `ToolError::CacheRoot` when `dest` has no parent component,
/// `ToolError::CacheCollision` when a unique sibling temp path could not be
/// chosen after the configured maximum number of attempts, and
/// `ToolError::AtomicMoveFailed` when the rename of the existing destination
/// to a sibling backup or the rename of the new tree into place fails (in
/// the latter case the previous tree is restored on a best-effort basis and
/// the in-progress copy is removed).
pub fn stage_and_install(staged: &Path, dest: &Path) -> Result<(), ToolError> {
    if !staged.is_dir() {
        return Err(ToolError::cache_io(
            "inspect staged directory",
            staged,
            io::Error::new(io::ErrorKind::InvalidInput, "staged path is not a directory"),
        ));
    }
    let Some(parent) = dest.parent() else {
        return Err(ToolError::CacheRoot(format!(
            "destination path has no parent: {}",
            dest.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create cache parent", parent, err))?;

    let install_dir =
        unique_sibling_dir(parent, dest.file_name().unwrap_or_else(|| OsStr::new("tool")))?;
    copy_dir_contents(staged, &install_dir)?;

    let backup = if dest.exists() {
        let backup = unique_sibling_path(parent, ".previous")?;
        fs::rename(dest, &backup).map_err(|err| ToolError::AtomicMoveFailed {
            from: dest.to_path_buf(),
            to: backup.clone(),
            source: err,
        })?;
        Some(backup)
    } else {
        None
    };

    match fs::rename(&install_dir, dest) {
        Ok(()) => {
            if let Some(backup) = backup {
                fs::remove_dir_all(&backup).map_err(|err| {
                    ToolError::cache_io("remove previous cache directory", backup, err)
                })?;
            }
            Ok(())
        }
        Err(source) => {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, dest);
            }
            let _ = fs::remove_dir_all(&install_dir);
            Err(ToolError::AtomicMoveFailed {
                from: install_dir,
                to: dest.to_path_buf(),
                source,
            })
        }
    }
}

fn unique_sibling_dir(parent: &Path, stem: impl AsRef<OsStr>) -> Result<PathBuf, ToolError> {
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let candidate = unique_sibling_path(parent, stem.as_ref())?;
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(ToolError::cache_io("create cache temp directory", candidate, err));
            }
        }
    }
    Err(ToolError::CacheCollision {
        parent: parent.to_path_buf(),
        stem: stem.as_ref().to_string_lossy().into_owned(),
    })
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), ToolError> {
    for entry in sorted_dir_entries(src)? {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| ToolError::cache_io("inspect staged entry", &src_path, err))?;
        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)
                .map_err(|err| ToolError::cache_io("create staged subdirectory", &dst_path, err))?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)
                .map_err(|err| ToolError::cache_io("copy staged file", &src_path, err))?;
        } else {
            return Err(ToolError::cache_io(
                "copy staged entry",
                &src_path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staged entries must be files or directories",
                ),
            ));
        }
    }
    Ok(())
}
