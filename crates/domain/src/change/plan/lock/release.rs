//! Release path for [`Stamp`].

use std::fs;
use std::path::Path;

use specify_error::Error;

use super::{Released, Stamp};

impl Stamp {
    /// Release the stamp if we own it. See [`Released`] for the three
    /// successful outcomes.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] if the stamp file exists but cannot be read,
    ///   or if removing it (when we own the PID) fails.
    /// - [`Error::Diag`] with code `stamp-malformed` when the stamp
    ///   contents are not a valid PID — the self-heal path should
    ///   reclaim deliberately rather than `release` clobbering blindly.
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
            Ok(pid) => Ok(Released::HeldByOther { pid }),
            Err(_) => Err(Error::Diag {
                code: "stamp-malformed",
                detail: format!(
                    "{}: contents are not a valid PID; refusing to clobber (run the L2.G self-heal path)",
                    path.display()
                ),
            }),
        }
    }
}
