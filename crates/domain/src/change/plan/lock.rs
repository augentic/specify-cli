//! Stateless PID stamp at `.specify/plan.lock` used by the short-lived
//! CLI driver-lock verbs.

use std::path::{Path, PathBuf};

mod acquire;
mod pid;
mod release;
mod status;

#[cfg(test)]
mod tests;

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
/// `specify change plan lock status`.
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// so the stamp is the only signalling channel available.
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
