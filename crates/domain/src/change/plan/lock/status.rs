//! Snapshot path for `specify change plan lock status`.

use std::fs;
use std::path::Path;

use specify_error::Error;

use super::pid::is_pid_alive;
use super::{Stamp, State};

impl Stamp {
    /// Snapshot the current stamp.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the stamp file exists but cannot be read.
    pub fn status(project_dir: &Path) -> Result<State, Error> {
        Self::status_with_liveness_check(project_dir, is_pid_alive)
    }

    /// Snapshot with an injected liveness predicate.
    ///
    /// # Errors
    ///
    /// Same as [`Self::status`].
    pub fn status_with_liveness_check<F>(
        project_dir: &Path, is_pid_alive: F,
    ) -> Result<State, Error>
    where
        F: Fn(u32) -> bool,
    {
        let path = Self::lockfile_path(project_dir);
        if !path.exists() {
            return Ok(State {
                held: false,
                pid: None,
                stale: None,
            });
        }
        let contents = fs::read_to_string(&path)?;
        contents.trim().parse::<u32>().map_or(
            Ok(State {
                held: false,
                pid: None,
                stale: Some(true),
            }),
            |pid| {
                let alive = is_pid_alive(pid);
                Ok(State {
                    held: alive,
                    pid: Some(pid),
                    stale: Some(!alive),
                })
            },
        )
    }
}
