//! Crash-safe writers shared by every `.specify/*.yaml` writer.
//!
//! Pattern: write to a temp file in the same parent, `sync_all`, then
//! `persist` (atomic rename on a single filesystem). Readers see either
//! the full previous content or the full new content, never a partial
//! write. Both helpers create the parent directory on demand.

use std::path::Path;

use serde::Serialize;
use specify_error::Error;

/// Serialise `value` as YAML (with a guaranteed trailing newline) and
/// atomically persist it at `path`. See module-level docs for the
/// atomicity envelope.
///
/// # Errors
///
/// Returns `Error::Yaml` if serialisation fails, or `Error::Io` if the
/// temp-file write or rename fails.
pub fn atomic_yaml_write<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let mut content = serde_saphyr::to_string(value)?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    atomic_bytes_write(path, content.as_bytes())
}

/// Atomically write `bytes` to `path`. Used for non-YAML writers (e.g.
/// the PID stamp in `.specify/plan.lock`) where the caller has already
/// produced the exact on-disk bytes.
///
/// # Errors
///
/// Returns `Error::Io` if the temp-file create / write / rename fails.
pub fn atomic_bytes_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(tmp.as_file_mut(), bytes)?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
}
