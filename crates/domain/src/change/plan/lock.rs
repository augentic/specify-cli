//! Advisory PID lock at `.specify/plan.lock` for the Layer 2 executor.
//!
//! Two primitives live here:
//!
//! - [`Guard`] — RAII guard that holds an OS-level `flock(2)`
//!   exclusive lock on `.specify/plan.lock` for its entire lifetime,
//!   removing the lockfile on drop. Sized for in-process, long-lived
//!   drivers (a future native `specify change plan run --loop`).
//! - [`Stamp`] — stateless PID-stamp helper used by the short-
//!   lived `specify change plan lock {acquire, release, status}` CLI verbs
//!   that drive the `/change:execute` agent-side loop. Each CLI
//!   invocation exits within milliseconds, so holding an `flock` is
//!   not an option; the stamp file persists on disk between calls and
//!   the holder's liveness is inferred by probing the stamped PID.
//!
//! Both are advisory only; semantics are unreliable on network
//! filesystems (NFS/SMB). Specify workspaces live on a local FS.
//!
//! # Portability caveats
//!
//! - On Unix the PID-liveness probe uses `kill(pid, 0)` from `libc`
//!   and treats `EPERM` as "alive" (the target exists but belongs to
//!   another user).
//! - On non-Unix platforms (Windows) the liveness probe is a
//!   conservative `true` — we never reclaim, which favours safety
//!   over recovery. Flock behaviour there is delegated to
//!   `std::fs::File::try_lock` (stable since Rust 1.89).
//! - `flock(2)` on macOS/Linux locks the underlying open file
//!   description, so two independent `open()` calls from the same
//!   process do serialize — the in-process tests exercise this via
//!   the PID-liveness override rather than relying on cross-thread
//!   flock semantics.

use std::fs::File;
use std::path::{Path, PathBuf};

mod acquire;
mod pid;
mod release;
mod status;

#[cfg(test)]
mod tests;

/// RAII guard for the `.specify/plan.lock` advisory lock.
///
/// Holds an exclusive `flock` on the lockfile for the lifetime of the
/// guard, and removes the lockfile on `Drop`. Construct via
/// [`Guard::acquire`] (production) or
/// [`Guard::acquire_with_liveness_check`] (tests).
#[derive(Debug)]
pub struct Guard {
    /// Held open for the lifetime of the guard so the OS-level
    /// `flock` is released when we drop. `Option` so `Drop` can
    /// explicitly take the file before deleting the path.
    pub(super) file: Option<File>,
    pub(super) path: PathBuf,
    pub(super) pid: u32,
    pub(super) reclaimed_stale_pid: Option<u32>,
}

impl Guard {
    /// PID written into the lockfile (always `std::process::id()`).
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// If the guard reclaimed a stale lock on acquire, the PID that
    /// had been recorded. `None` for a cold acquire or when the
    /// previous contents were malformed (no PID to report).
    ///
    /// `/change:execute` renders this in its preamble as
    /// "reclaimed stale lock from PID X".
    #[must_use]
    pub const fn reclaimed_stale_pid(&self) -> Option<u32> {
        self.reclaimed_stale_pid
    }

    /// Absolute path of the lockfile this guard manages.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Result of a successful [`Stamp::acquire`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// verbatim via `specify change plan lock release --format json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanLockReleased {
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
    /// intact. `pid` is `None` when the file contents were malformed.
    HeldByOther {
        /// PID of the other holder, if parseable.
        pid: Option<u32>,
    },
}

/// Snapshot of the on-disk `.specify/plan.lock` stamp, as reported by
/// `specify change plan lock status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLockState {
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
/// Unlike [`Guard`], this primitive does **not** hold an
/// OS-level advisory lock. It manages `.specify/plan.lock` as a
/// persistent PID marker that survives the process writing it:
///
/// - `specify change plan lock acquire --pid <P>` stamps `P` into the file
///   (failing with [`specify_error::Error::DriverBusy`] when another
///   live PID holds it).
/// - `specify change plan lock release --pid <P>` removes the file when it
///   still holds `P`; refuses when it holds another PID (stale locks
///   are reclaimed by the L2.G self-heal path, not by release).
/// - `specify change plan lock status` reports the current holder (if any)
///   and whether the stamp is considered stale.
///
/// The `/change:execute` skill calls these verbs around its agent-side
/// loop; no Rust-level process stays alive for the full driver run,
/// so the stamp is the only signalling channel available. Secondary
/// protection against genuine same-process racing is provided by
/// [`Guard`], which future long-lived drivers can wrap around
/// a stamped run.
#[derive(Debug)]
pub struct Stamp;

impl Stamp {
    /// Path to the `.specify/plan.lock` stamp file.
    ///
    /// Shared by `acquire` / `release` / `status` so they all agree on
    /// the same on-disk location.
    pub(super) fn lockfile_path(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify").join("plan.lock")
    }
}
