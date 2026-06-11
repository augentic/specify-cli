//! Plan-lock probe (RFC-44 R2 runtime enforcement).
//!
//! The `/spec:execute` driver lock is the OS advisory lock on
//! `<plan-root>/.specify/plan.lock`, **acquired skill-side** by the
//! `flock`-based snippet in the framework's `plan-lock.md` — it is
//! deliberately not a CLI verb. This module is the read side: the
//! plan-state-writing verbs (`plan next`, per-entry `plan transition`,
//! `slice merge run`) probe the lock and refuse an unlocked driver
//! with `plan-lock-not-held` (exit 2), so dual-driving refusal is a
//! runtime property instead of a per-skill snippet discipline.

use std::fs::File;
use std::path::Path;

use specify_error::Error;

use crate::config::Layout;

/// Probe outcome for the plan lock at one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockProbe {
    /// Some process holds the lock — a driver session is active.
    Held,
    /// Nobody holds the lock (including: the lockfile does not exist).
    Unheld,
}

/// Refuse the calling verb unless a driver session holds the plan lock.
///
/// # Errors
///
/// [`Error::Validation`] `plan-lock-not-held` (exit 2) when the probe
/// reports [`LockProbe::Unheld`]; I/O failures from the probe itself.
pub fn require_held(layout: Layout<'_>) -> Result<(), Error> {
    let path = layout.plan_lock_path();
    match probe(&path)? {
        LockProbe::Held => Ok(()),
        LockProbe::Unheld => Err(Error::validation_failed(
            "plan-lock-not-held",
            "no driver session holds the plan lock",
            format!(
                "acquire {} with the flock snippet in plan-lock.md before driving plan state \
                 (every /spec:execute and breakout session holds it for the session's lifetime)",
                path.display()
            ),
        )),
    }
}

/// Probe whether any process holds an exclusive advisory lock on
/// `path`. A missing lockfile is [`LockProbe::Unheld`] — no driver
/// session ever created it.
///
/// Both advisory-lock families are covered, because the blessed
/// snippets differ per platform and on Linux the two families do not
/// interact: an `fcntl(2)` record-lock query (`F_GETLK` — read-only,
/// acquires nothing) first, then a `flock(2)` try-acquire on a fresh
/// descriptor (`LOCK_EX | LOCK_NB`, released immediately on success).
///
/// # Errors
///
/// Propagates I/O failures other than a missing file; a `flock` /
/// `fcntl` failure other than "would block" surfaces as [`Error::Io`].
pub fn probe(path: &Path) -> Result<LockProbe, Error> {
    let file = match File::options().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(LockProbe::Unheld),
        Err(err) => return Err(Error::Io(err)),
    };
    imp::probe_open(&file)
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;

    use specify_error::Error;

    use super::LockProbe;

    /// Probe an open lockfile descriptor: `F_GETLK` query, then
    /// `flock` try-acquire.
    pub(super) fn probe_open(file: &File) -> Result<LockProbe, Error> {
        if fcntl_lock_present(file)? {
            return Ok(LockProbe::Held);
        }
        flock_try_acquire(file)
    }

    /// `fcntl(F_GETLK)` with a whole-file write-lock probe. Reports
    /// whether a conflicting record lock exists without acquiring
    /// anything. Covers fcntl-family snippets (e.g. zsh
    /// `zsystem flock`), which `flock(2)` cannot see on Linux.
    #[expect(
        unsafe_code,
        reason = "fcntl(F_GETLK) has no std wrapper; std's File locking is flock-family only"
    )]
    fn fcntl_lock_present(file: &File) -> Result<bool, Error> {
        let mut probe = libc::flock {
            l_start: 0,
            l_len: 0,
            l_pid: 0,
            l_type: libc::F_WRLCK,
            // SEEK_SET; spelled as the literal to keep the i32 const
            // out of the platform-varying c_short field.
            l_whence: 0,
            #[cfg(target_os = "freebsd")]
            l_sysid: 0,
        };
        // SAFETY: `file` owns a valid open descriptor for the duration
        // of the call and `probe` is a properly initialised flock
        // struct the kernel only writes back into.
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &raw mut probe) };
        if rc == -1 {
            return Err(Error::Io(io::Error::last_os_error()));
        }
        Ok(probe.l_type != libc::F_UNLCK)
    }

    /// `flock`-family try-acquire (std's `File::try_lock`) on the
    /// caller's fresh descriptor: success means nobody held it (the
    /// probe lock is released immediately); would-block means a driver
    /// holds it.
    fn flock_try_acquire(file: &File) -> Result<LockProbe, Error> {
        match file.try_lock() {
            Ok(()) => {
                file.unlock().map_err(Error::Io)?;
                Ok(LockProbe::Unheld)
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(LockProbe::Held),
            Err(std::fs::TryLockError::Error(err)) => Err(Error::Io(err)),
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::fs::File;

    use specify_error::Error;

    use super::LockProbe;

    /// No advisory-lock probe off Unix: report `Held` so enforcement
    /// degrades to permissive rather than bricking every driver verb.
    pub(super) fn probe_open(_file: &File) -> Result<LockProbe, Error> {
        Ok(LockProbe::Held)
    }
}

#[cfg(test)]
mod tests;
