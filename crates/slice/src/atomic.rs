//! Crash-safe "write a temp file, rename into place" helpers shared by
//! every `.specify/*.yaml` writer in the crate.
//!
//! The pattern is `NamedTempFile::new_in(parent)`, `write_all`,
//! `sync_all`, then `persist`, which bottoms out at `fs::rename`
//! (atomic on a single filesystem). Readers observe either the
//! previous complete contents or the new complete contents, never a
//! half-written or empty file.
//!
//! Both helpers create the parent directory on demand so callers can
//! write to a freshly-minted `.specify/` layout without an explicit
//! `create_dir_all` at every call site.
//!
//! Promoted from `pub(crate)` to `pub` by RFC-13 chunk 2.4 so the
//! lifted plan + lock primitives in `specify-initiative` can route
//! their on-disk writes through the same atomic-rename envelope as
//! the per-loop-unit primitives that remain in this crate. Downstream
//! library users who need the same behaviour should call the domain
//! helpers (`ChangeMetadata::save`, `Journal::append`, plus the
//! `Plan::save` / `Stamp::acquire` re-exports in `specify-initiative`)
//! that route through here.

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
