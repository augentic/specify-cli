//! `.gitignore` upkeep for `.specify/`-internal directories.
//!
//! Lifted from `src/init.rs` by RFC-13 chunk 2.2 so the registry crate
//! (which now owns `.specify/workspace/` materialisation) can enforce
//! the convention without reaching back into the binary lib.
//!
//! `init` and `specify workspace sync` both call
//! [`ensure_specify_gitignore_entries`] directly; RFC-13 chunk 2.3 dropped
//! the temporary `specify::ensure_specify_gitignore_entries` re-export
//! from the binary lib.

use std::fs;
use std::path::Path;

use specify_error::Error;

/// Lines the framework requires in the project `.gitignore`. Both
/// directories are framework-managed scratch under `.specify/` and
/// must never be checked in.
const SPECIFY_GITIGNORE_ENTRIES: &[&str] = &[".specify/.cache/", ".specify/workspace/"];

/// Idempotent: ensure each line in `SPECIFY_GITIGNORE_ENTRIES` appears
/// exactly once (matched with `trim()` per line) in the project
/// `.gitignore`, appending missing lines with a trailing newline.
///
/// Used by `specify init` and by `specify workspace sync` (RFC-3a
/// C29).
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn ensure_specify_gitignore_entries(project_dir: &Path) -> Result<(), Error> {
    let path = project_dir.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(Error::Io(err)),
    };

    let mut updated = existing;
    let mut changed = false;
    for entry in SPECIFY_GITIGNORE_ENTRIES {
        if updated.lines().any(|line| line.trim() == *entry) {
            continue;
        }
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(entry);
        updated.push('\n');
        changed = true;
    }

    if changed {
        fs::write(&path, updated)?;
    }
    Ok(())
}
