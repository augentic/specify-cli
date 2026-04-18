//! Advisory PID lock at `.specify/plan.lock` for the Layer 2 executor.
//!
//! See RFC-2 §"Driver Concurrency". Exclusive (`flock`-style) lock held
//! for the lifetime of a [`PlanLockGuard`]. Stale locks (PID no longer
//! alive, or malformed lockfile contents) are reclaimed on acquire.
//!
//! Advisory only; semantics are unreliable on network filesystems
//! (NFS/SMB). Specify workspaces live on a local FS, per RFC-2.
//!
//! # Portability caveats
//!
//! - On Unix the PID-liveness probe uses `kill(pid, 0)` from `libc`
//!   and treats `EPERM` as "alive" (the target exists but belongs to
//!   another user).
//! - On non-Unix platforms (Windows) the liveness probe is a
//!   conservative `true` — we never reclaim, which favours safety
//!   over recovery. Flock behaviour there is delegated to `fs2`.
//! - `flock(2)` on macOS/Linux locks the underlying open file
//!   description, so two independent `open()` calls from the same
//!   process do serialize — the in-process tests exercise this via
//!   the PID-liveness override rather than relying on cross-thread
//!   flock semantics.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use specify_error::Error;

/// RAII guard for the `.specify/plan.lock` advisory lock.
///
/// Holds an exclusive `flock` on the lockfile for the lifetime of the
/// guard, and removes the lockfile on `Drop`. Construct via
/// [`PlanLockGuard::acquire`] (production) or
/// [`PlanLockGuard::acquire_with_liveness_check`] (tests).
#[derive(Debug)]
pub struct PlanLockGuard {
    /// Held open for the lifetime of the guard so the OS-level
    /// `flock` is released when we drop. `Option` so `Drop` can
    /// explicitly take the file before deleting the path.
    file: Option<File>,
    path: PathBuf,
    pid: u32,
    reclaimed_stale_pid: Option<u32>,
}

impl PlanLockGuard {
    /// Acquire the lock using the real OS-level PID-liveness probe.
    ///
    /// Returns `Err(Error::DriverBusy { pid })` if another live driver
    /// holds the lock. Reclaims the lock silently if the recorded PID
    /// is dead or the lockfile contents are malformed.
    pub fn acquire(project_dir: &Path) -> Result<Self, Error> {
        Self::acquire_with_liveness_check(project_dir, is_pid_alive)
    }

    /// Acquire with an injected PID-liveness predicate. Exposed so
    /// tests can force "alive" / "dead" outcomes deterministically
    /// without spawning child processes.
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

        let file = OpenOptions::new().write(true).create(true).truncate(true).open(&path)?;

        // Use fs2's extension method explicitly to avoid ambiguity
        // with std's inherent `File::try_lock_exclusive` (stable
        // since Rust 1.89). Both behave the same for our purposes,
        // but pinning to fs2 keeps the API pre-1.89 compatible if we
        // ever lower MSRV.
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Another process grabbed the flock between our
                // existence check and open(). Re-read the PID so the
                // error names the winner.
                let contents = fs::read_to_string(&path).unwrap_or_default();
                let pid = contents.trim().parse::<u32>().unwrap_or(0);
                return Err(Error::DriverBusy { pid });
            }
            Err(e) => return Err(Error::Io(e)),
        }

        let pid = std::process::id();
        let mut writer = &file;
        writer.write_all(pid.to_string().as_bytes())?;
        writer.flush()?;
        file.sync_all()?;

        Ok(PlanLockGuard {
            file: Some(file),
            path,
            pid,
            reclaimed_stale_pid,
        })
    }

    /// PID written into the lockfile (always `std::process::id()`).
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// If the guard reclaimed a stale lock on acquire, the PID that
    /// had been recorded. `None` for a cold acquire or when the
    /// previous contents were malformed (no PID to report).
    ///
    /// `/spec:execute` renders this in its preamble as
    /// "reclaimed stale lock from PID X".
    pub fn reclaimed_stale_pid(&self) -> Option<u32> {
        self.reclaimed_stale_pid
    }

    /// Absolute path of the lockfile this guard manages.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PlanLockGuard {
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

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    // SAFETY: `kill(pid, 0)` is a liveness probe with no side
    // effects. It returns `0` when the process exists and the
    // caller has permission to signal it. `EPERM` means the
    // process exists but is owned by another user — still alive.
    // `ESRCH` means no such process.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    // Conservative default on non-Unix: assume any recorded PID is
    // still live. This trades reclaim recovery for safety —
    // operators on Windows will see `DriverBusy` until they remove
    // the stale lockfile by hand.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    fn read_lock_pid(dir: &Path) -> String {
        fs::read_to_string(dir.join(".specify").join("plan.lock")).expect("read lockfile")
    }

    #[test]
    fn acquire_and_release_creates_and_removes_lockfile() {
        let dir = tempdir().expect("tempdir");
        let guard = PlanLockGuard::acquire(dir.path()).expect("acquire ok");

        let lock_path = dir.path().join(".specify").join("plan.lock");
        assert!(lock_path.exists(), "lockfile should exist while guard is held");
        assert_eq!(read_lock_pid(dir.path()).trim(), std::process::id().to_string());
        assert_eq!(guard.pid(), std::process::id());
        assert_eq!(guard.reclaimed_stale_pid(), None);

        drop(guard);
        assert!(!lock_path.exists(), "lockfile should be removed on drop");
    }

    #[test]
    fn second_acquire_while_first_held_returns_driver_busy() {
        let dir = tempdir().expect("tempdir");
        let _first =
            PlanLockGuard::acquire_with_liveness_check(dir.path(), |_| true).expect("first ok");

        let err = PlanLockGuard::acquire_with_liveness_check(dir.path(), |_| true)
            .expect_err("second should fail");
        match err {
            Error::DriverBusy { pid } => assert_eq!(pid, std::process::id()),
            other => panic!("expected DriverBusy, got {other:?}"),
        }
    }

    #[test]
    fn stale_lock_with_dead_pid_is_reclaimed() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
        fs::write(dir.path().join(".specify").join("plan.lock"), "99999").expect("prime stale");

        let guard =
            PlanLockGuard::acquire_with_liveness_check(dir.path(), |_| false).expect("reclaim ok");
        assert_eq!(guard.reclaimed_stale_pid(), Some(99999));
        assert_eq!(read_lock_pid(dir.path()).trim(), std::process::id().to_string());
    }

    #[test]
    fn malformed_pid_in_lockfile_is_reclaimed() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
        fs::write(dir.path().join(".specify").join("plan.lock"), "not-a-number\n")
            .expect("prime malformed");

        let guard =
            PlanLockGuard::acquire_with_liveness_check(dir.path(), |_| true).expect("reclaim ok");
        assert_eq!(
            guard.reclaimed_stale_pid(),
            None,
            "malformed contents carry no prior PID to report"
        );
        assert_eq!(read_lock_pid(dir.path()).trim(), std::process::id().to_string());
    }

    #[test]
    fn guard_drop_removes_lockfile_even_on_panic() {
        let dir = tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();
        let lock_path = dir_path.join(".specify").join("plan.lock");

        let result = std::panic::catch_unwind(|| {
            let _guard = PlanLockGuard::acquire(&dir_path).expect("acquire ok");
            panic!("simulated failure while holding lock");
        });
        assert!(result.is_err(), "inner closure should have panicked");
        assert!(!lock_path.exists(), "lockfile should be cleaned on unwind");
    }

    #[test]
    fn reclaim_logs_diagnostic_via_reclaimed_stale_pid() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
        fs::write(dir.path().join(".specify").join("plan.lock"), "99999").expect("prime stale");

        let guard =
            PlanLockGuard::acquire_with_liveness_check(dir.path(), |_| false).expect("reclaim ok");

        assert_eq!(guard.reclaimed_stale_pid(), Some(99999));
    }

    #[test]
    fn second_acquire_in_different_thread_while_first_held_returns_driver_busy() {
        // Cross-thread acquisition is verified via the liveness
        // override rather than raw flock semantics, which per the
        // module-level doc comment we consider belt-plus-PID-file.
        let dir = tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let holder_dir = dir_path.clone();
        let holder = thread::spawn(move || {
            let guard = PlanLockGuard::acquire_with_liveness_check(&holder_dir, |_| true)
                .expect("holder acquire ok");
            started_tx.send(()).expect("notify started");
            release_rx.recv().expect("await release signal");
            drop(guard);
        });

        started_rx.recv().expect("holder started");

        let err = PlanLockGuard::acquire_with_liveness_check(&dir_path, |_| true)
            .expect_err("contender should see DriverBusy");
        assert!(matches!(err, Error::DriverBusy { .. }));

        release_tx.send(()).expect("release holder");
        holder.join().expect("holder joined");

        // After release, a fresh acquire should succeed.
        thread::sleep(Duration::from_millis(10));
        let _after = PlanLockGuard::acquire_with_liveness_check(&dir_path, |_| true)
            .expect("post-release acquire ok");
    }
}
