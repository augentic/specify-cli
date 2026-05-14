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
