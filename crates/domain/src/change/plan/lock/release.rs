//! Release paths: [`Stamp::release`] for the CLI verb and
//! [`impl Drop for Guard`] for the in-process RAII holder.

use std::fs;
use std::path::Path;

use specify_error::Error;

use super::{Guard, Released, Stamp};

impl Stamp {
    /// Release the stamp if we own it. See [`Released`] for
    /// the four outcomes.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the stamp file exists but cannot be read, or
    /// if removing it (when we own the PID) fails.
    pub fn release(project_dir: &Path, our_pid: u32) -> Result<Released, Error> {
        let path = Self::lockfile_path(project_dir);
        if !path.exists() {
            return Ok(Released::WasAbsent);
        }
        let contents = fs::read_to_string(&path)?;
        match contents.trim().parse::<u32>() {
            Ok(pid) if pid == our_pid => {
                fs::remove_file(&path)?;
                Ok(Released::Removed { pid })
            }
            Ok(pid) => Ok(Released::HeldByOther { pid: Some(pid) }),
            Err(_) => Ok(Released::HeldByOther { pid: None }),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Drop the `File` first so the OS releases the advisory
        // lock before anyone else can observe a file with a missing
        // lock.
        self.file.take();
        // `NotFound` is benign — another process or a test helper
        // may have cleaned up already. Any other error is swallowed
        // rather than panicking from `Drop`.
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            // Best-effort: surface on stderr but don't panic from Drop.
            eprintln!("warning: failed to remove plan lock at {}: {e}", self.path.display());
        }
    }
}
