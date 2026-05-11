//! Acquire paths for [`Guard`] (OS-level `flock`) and [`Stamp`]
//! (PID-only marker).

use std::fs::{self, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;

use specify_error::Error;
use crate::slice::atomic::bytes_write;

use super::pid::is_pid_alive;
use super::{Acquired, Guard, Stamp};

impl Guard {
    /// Acquire the lock using the real OS-level PID-liveness probe.
    ///
    /// Reclaims the lock silently if the recorded PID is dead or the
    /// lockfile contents are malformed.
    ///
    /// # Errors
    ///
    /// - [`Error::DriverBusy`] when another live driver holds the lock.
    /// - [`Error::Io`] if `.specify/` cannot be created, the lockfile
    ///   cannot be opened, the OS-level `flock` syscall fails for
    ///   reasons other than `WouldBlock`, or the PID stamp cannot be
    ///   written and synced to disk.
    pub fn acquire(project_dir: &Path) -> Result<Self, Error> {
        Self::acquire_with_liveness_check(project_dir, is_pid_alive)
    }

    /// Acquire with an injected PID-liveness predicate. Exposed so
    /// tests can force "alive" / "dead" outcomes deterministically
    /// without spawning child processes.
    ///
    /// # Errors
    ///
    /// Same as [`Self::acquire`].
    pub fn acquire_with_liveness_check<F>(
        project_dir: &Path, is_pid_alive: F,
    ) -> Result<Self, Error>
    where
        F: Fn(u32) -> bool,
    {
        let specify_dir = project_dir.join(".specify");
        fs::create_dir_all(&specify_dir)?;
        let path = specify_dir.join("plan.lock");

        let mut reclaimed_stale_pid: Option<u32> = None;

        if path.exists() {
            let contents = fs::read_to_string(&path).unwrap_or_default();
            match contents.trim().parse::<u32>() {
                Ok(pid) if is_pid_alive(pid) => {
                    return Err(Error::DriverBusy { pid });
                }
                Ok(pid) => {
                    reclaimed_stale_pid = Some(pid);
                }
                Err(_) => {
                    // Malformed contents — treat as stale. Nothing to
                    // surface on `reclaimed_stale_pid` because there is
                    // no valid PID to report.
                }
            }
        }

        // Open without `truncate(true)`: truncation before the flock
        // would leave a zero-length lockfile observable to any reader
        // that `open`s between our `set_len(0)` and the `flock`.
        // Truncation is deferred until we hold the exclusive lock via
        // the `file.set_len(0)` call below.
        let file = OpenOptions::new().write(true).create(true).truncate(false).open(&path)?;

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                // Another process grabbed the flock between our
                // existence check and open(). Re-read the PID so the
                // error names the winner.
                let contents = fs::read_to_string(&path).unwrap_or_default();
                let pid = contents.trim().parse::<u32>().unwrap_or(0);
                return Err(Error::DriverBusy { pid });
            }
            Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
        }

        // Now that we hold the flock, it is safe to rewrite the PID:
        // no other writer can observe a partially-written file.
        let pid = std::process::id();
        file.set_len(0)?;
        let mut writer = &file;
        writer.write_all(pid.to_string().as_bytes())?;
        writer.flush()?;
        file.sync_all()?;

        Ok(Self {
            file: Some(file),
            path,
            pid,
            reclaimed_stale_pid,
        })
    }
}

impl Stamp {
    /// Acquire the stamp using the real PID-liveness probe. See
    /// [`Self::acquire_with_liveness_check`] for the full semantics.
    ///
    /// # Errors
    ///
    /// - [`Error::DriverBusy`] when another live PID is already
    ///   stamped in `.specify/plan.lock`.
    /// - [`Error::Io`] if `.specify/` cannot be created, the existing
    ///   stamp cannot be read, or the new stamp cannot be atomically
    ///   written.
    pub fn acquire(project_dir: &Path, our_pid: u32) -> Result<Acquired, Error> {
        Self::acquire_with_liveness_check(project_dir, our_pid, is_pid_alive)
    }

    /// Acquire with an injected liveness predicate. Exposed so tests
    /// can assert `DriverBusy` vs reclaim without relying on a
    /// particular host PID being alive.
    ///
    /// # Errors
    ///
    /// Same as [`Self::acquire`].
    pub fn acquire_with_liveness_check<F>(
        project_dir: &Path, our_pid: u32, is_pid_alive: F,
    ) -> Result<Acquired, Error>
    where
        F: Fn(u32) -> bool,
    {
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
