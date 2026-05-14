//! Acquire path for [`Stamp`] (PID-only marker).

use std::fs;
use std::path::Path;

use specify_error::Error;

use super::pid::is_pid_alive;
use super::{Acquired, Stamp};
use crate::slice::atomic::bytes_write;

impl Stamp {
    /// Acquire the stamp for `our_pid`.
    ///
    /// Reclaims the stamp silently if the recorded PID is dead or the
    /// stamp contents are malformed.
    ///
    /// # Errors
    ///
    /// - [`Error::DriverBusy`] when another live PID is already
    ///   stamped in `.specify/plan.lock`.
    /// - [`Error::Io`] if `.specify/` cannot be created, the existing
    ///   stamp cannot be read, or the new stamp cannot be atomically
    ///   written.
    pub fn acquire(project_dir: &Path, our_pid: u32) -> Result<Acquired, Error> {
        let specify_dir = project_dir.join(".specify");
        fs::create_dir_all(&specify_dir)?;
        let path = Self::lockfile_path(project_dir);

        let mut reclaimed_stale_pid: Option<u32> = None;
        let mut already_held = false;

        if path.exists() {
            let contents = fs::read_to_string(&path).unwrap_or_default();
            match contents.trim().parse::<u32>() {
                Ok(pid) if pid == our_pid => {
                    already_held = true;
                }
                Ok(pid) if is_pid_alive(pid) => {
                    return Err(Error::DriverBusy { pid });
                }
                Ok(pid) => {
                    reclaimed_stale_pid = Some(pid);
                }
                Err(_) => {
                    // Malformed — treat as stale. No PID to surface.
                }
            }
        }

        // Atomic write via tempfile + rename, matching the convention
        // used by `Plan::save` and `SliceMetadata::save`. Readers
        // never observe a partial stamp.
        bytes_write(&path, our_pid.to_string().as_bytes())?;

        Ok(Acquired {
            pid: our_pid,
            reclaimed_stale_pid,
            already_held,
        })
    }
}
