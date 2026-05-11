//! `.gitignore` upkeep for `.specify/`-internal directories.
//!
//! The registry crate owns `.specify/workspace/` materialisation and so
//! enforces the convention here. `init` and `specify workspace sync`
//! both call [`ensure_specify_gitignore_entries`] directly.

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
/// Used by `specify init` and by `specify workspace sync`.
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
