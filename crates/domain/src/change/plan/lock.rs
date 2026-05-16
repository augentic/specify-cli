//! Stateless PID stamp at `.specify/plan.lock` used by the short-lived
//! CLI driver-lock verbs.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_error::Error;

use crate::slice::atomic::bytes_write;

#[cfg(test)]
mod tests;

/// Result of a successful [`Stamp::acquire`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Acquired {
    /// PID written into the stamp file.
    pub pid: u32,
    /// If the acquire reclaimed a stale stamp, the PID that had been
    /// recorded. `None` for a cold acquire, a re-stamp of our own PID,
    /// or when the previous contents were malformed (no valid PID to
    /// report).
    pub reclaimed_stale_pid: Option<u32>,
    /// `true` when the file already contained our PID — the acquire
    /// was a no-op re-stamp rather than a fresh take.
    pub already_held: bool,
}

/// Outcome of a [`Stamp::release`] call. The CLI surfaces this
/// verbatim via `specify plan lock release --format json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released {
    /// Stamp file was present and held our PID — now removed.
    Removed {
        /// PID that was in the stamp file.
        pid: u32,
    },
    /// Stamp file was absent — nothing to do.
    WasAbsent,
    /// Stamp file was present but held a PID that isn't ours. We
    /// refuse to clobber it so a concurrent driver (or a stale stamp
    /// that the self-heal path should reclaim deliberately) stays
    /// intact.
    HeldByOther {
        /// PID of the other holder.
        pid: u32,
    },
}

/// Snapshot of the on-disk `.specify/plan.lock` stamp, as reported by
/// `specify plan lock status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct State {
    /// `true` when the stamp file exists and the stamped PID is
    /// considered alive by the host liveness probe.
    pub held: bool,
    /// PID currently stamped in `.specify/plan.lock`, if any. `None`
    /// when the file is absent or malformed.
    pub pid: Option<u32>,
    /// `true` when the stamp file exists but the stamped PID is dead
    /// or the contents are malformed. `None` when the file is absent.
    pub stale: Option<bool>,
}

/// PID-stamp helper for the short-lived CLI driver-lock protocol.
///
/// This primitive manages `.specify/plan.lock` as a persistent PID
/// marker that survives the process writing it:
///
/// - `specify plan lock acquire --pid <P>` stamps `P` into the file
///   (failing with [`specify_error::Error::Diag`] code `driver-busy`
///   when another live PID holds it).
/// - `specify plan lock release --pid <P>` removes the file when it
///   still holds `P`; refuses when it holds another PID (stale locks
///   are reclaimed by the L2.G self-heal path, not by release).
/// - `specify plan lock status` reports the current holder (if any)
///   and whether the stamp is considered stale.
///
/// The `/change:execute` skill calls these verbs around its agent-side
/// loop; no Rust-level process stays alive for the full driver run,
/// so the stamp is the only signalling channel available.
#[derive(Debug, Clone, Copy)]
pub struct Stamp;

impl Stamp {
    /// Path to the `.specify/plan.lock` stamp file.
    ///
    /// Shared by `acquire` / `release` / `status` so they all agree on
    /// the same on-disk location.
    pub(super) fn lockfile_path(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify").join("plan.lock")
    }

    /// Acquire the stamp for `our_pid`.
    ///
    /// Reclaims the stamp silently if the recorded PID is dead or the
    /// stamp contents are malformed.
    ///
    /// # Errors
    ///
    /// - [`Error::Diag`] with code `driver-busy` when another live PID
    ///   is already stamped in `.specify/plan.lock`.
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
                    return Err(Error::Diag {
                        code: "driver-busy",
                        detail: format!(
                            "another /change:execute driver is running (pid {pid}); refusing to proceed"
                        ),
                    });
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

/// PID-liveness probe. Unix uses `kill(pid, 0)` (treating `EPERM` as
/// alive); non-Unix is a conservative `true` so we never reclaim.
#[cfg(unix)]
#[expect(unsafe_code, reason = "libc::kill(pid, 0) is the canonical Unix liveness probe")]
fn is_pid_alive(pid: u32) -> bool {
    // SAFETY: `kill(pid, 0)` is a liveness probe with no side
    // effects. It returns `0` when the process exists and the
    // caller has permission to signal it. `EPERM` means the
    // process exists but is owned by another user — still alive.
    // `ESRCH` means no such process.
    let rc = unsafe { libc::kill(pid.cast_signed(), 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    // Conservative default on non-Unix: assume any recorded PID is
    // still live. This trades reclaim recovery for safety —
    // operators on Windows will see the `driver-busy` diagnostic
    // until they remove the stale lockfile by hand.
    true
}
