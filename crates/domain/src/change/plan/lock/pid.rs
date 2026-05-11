//! PID-liveness probe used by both [`super::Guard`] and [`super::Stamp`].
//!
//! On Unix the probe uses `kill(pid, 0)` and treats `EPERM` as "alive"
//! (the target exists but belongs to another user). On non-Unix
//! platforms (Windows) the probe is a conservative `true` — we never
//! reclaim, which favours safety over recovery.

#[cfg(unix)]
pub(super) fn is_pid_alive(pid: u32) -> bool {
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
pub(super) fn is_pid_alive(_pid: u32) -> bool {
    // Conservative default on non-Unix: assume any recorded PID is
    // still live. This trades reclaim recovery for safety —
    // operators on Windows will see `DriverBusy` until they remove
    // the stale lockfile by hand.
    true
}
