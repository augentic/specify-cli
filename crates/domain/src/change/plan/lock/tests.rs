use std::path::Path;
use std::{fs, process};

use specify_error::Error;
use tempfile::tempdir;

use super::*;

fn read_lock_pid(dir: &Path) -> String {
    fs::read_to_string(dir.join(".specify").join("plan.lock")).expect("read lockfile")
}

fn prime_stamp(dir: &Path, contents: &str) {
    fs::create_dir_all(dir.join(".specify")).expect("mkdir .specify");
    fs::write(dir.join(".specify").join("plan.lock"), contents).expect("prime stamp");
}

#[test]
fn stamp_acquire_release() {
    let dir = tempdir().expect("tempdir");
    let pid = process::id();
    let acquired = Stamp::acquire(dir.path(), pid).expect("acquire ok");
    assert_eq!(acquired.pid, pid);
    assert_eq!(acquired.reclaimed_stale_pid, None);
    assert!(!acquired.already_held);
    assert_eq!(read_lock_pid(dir.path()).trim(), pid.to_string());

    let released = Stamp::release(dir.path(), pid).expect("release ok");
    assert_eq!(released, Released::Removed { pid });
    assert!(!dir.path().join(".specify").join("plan.lock").exists());
}

#[test]
fn stamp_reacquire_idempotent() {
    let dir = tempdir().expect("tempdir");
    let pid = process::id();
    Stamp::acquire(dir.path(), pid).expect("first");
    let again = Stamp::acquire(dir.path(), pid).expect("reacquire ok");
    assert!(again.already_held, "same-PID re-stamp must report already_held");
    assert_eq!(again.reclaimed_stale_pid, None);
}

#[test]
fn stamp_acquire_busy() {
    let dir = tempdir().expect("tempdir");
    // Prime with our own PID — the real liveness probe will find it
    // alive (this process is running) and refuse to let a contender
    // take over.
    let live_pid = process::id();
    prime_stamp(dir.path(), &live_pid.to_string());

    // Pick any PID that isn't this process's PID.
    let contender = if live_pid == 1 { 2 } else { 1 };
    let err = Stamp::acquire(dir.path(), contender).expect_err("expected DriverBusy");
    assert!(matches!(err, Error::DriverBusy { pid } if pid == live_pid));
    // Contents unchanged — we never clobbered the live holder.
    assert_eq!(read_lock_pid(dir.path()).trim(), live_pid.to_string());
}

#[test]
fn stamp_reclaims_stale() {
    let dir = tempdir().expect("tempdir");
    // PID 0 / very large PIDs are not allocated on any host the test
    // suite runs on; `kill(pid, 0)` returns `ESRCH` and reclaim wins.
    prime_stamp(dir.path(), "99999999");

    let pid = process::id();
    let acquired = Stamp::acquire(dir.path(), pid).expect("reclaim ok");
    assert_eq!(acquired.reclaimed_stale_pid, Some(99_999_999));
    assert_eq!(read_lock_pid(dir.path()).trim(), pid.to_string());
}

#[test]
fn stamp_reclaims_malformed() {
    let dir = tempdir().expect("tempdir");
    prime_stamp(dir.path(), "not-a-pid\n");

    let pid = process::id();
    let acquired = Stamp::acquire(dir.path(), pid).expect("reclaim ok");
    assert_eq!(
        acquired.reclaimed_stale_pid, None,
        "malformed contents carry no prior PID to report"
    );
    assert_eq!(read_lock_pid(dir.path()).trim(), pid.to_string());
}

#[test]
fn stamp_release_absent() {
    let dir = tempdir().expect("tempdir");
    let released = Stamp::release(dir.path(), 4242).expect("release ok");
    assert_eq!(released, Released::WasAbsent);
}

#[test]
fn stamp_release_refuses_other() {
    let dir = tempdir().expect("tempdir");
    prime_stamp(dir.path(), "7777");

    let released = Stamp::release(dir.path(), 4242).expect("release ok");
    assert_eq!(released, Released::HeldByOther { pid: 7777 });
    // File still there — we refused to clobber.
    assert_eq!(read_lock_pid(dir.path()).trim(), "7777");
}

#[test]
fn stamp_release_malformed_diag() {
    let dir = tempdir().expect("tempdir");
    prime_stamp(dir.path(), "not-a-pid\n");

    let err = Stamp::release(dir.path(), 4242).expect_err("expected stamp-malformed Diag");
    match err {
        Error::Diag { code, .. } => assert_eq!(code, "stamp-malformed"),
        other => panic!("expected Error::Diag, got {other:?}"),
    }
    // File untouched.
    assert_eq!(read_lock_pid(dir.path()).trim(), "not-a-pid");
}

#[test]
fn stamp_status_absent() {
    let dir = tempdir().expect("tempdir");
    let state = Stamp::status(dir.path()).expect("status ok");
    assert_eq!(
        state,
        State {
            held: false,
            pid: None,
            stale: None
        }
    );
}

#[test]
fn stamp_status_held() {
    let dir = tempdir().expect("tempdir");
    let pid = process::id();
    Stamp::acquire(dir.path(), pid).expect("acquire");

    let state = Stamp::status(dir.path()).expect("status ok");
    assert_eq!(
        state,
        State {
            held: true,
            pid: Some(pid),
            stale: Some(false)
        }
    );
}

#[test]
fn stamp_status_stale() {
    let dir = tempdir().expect("tempdir");
    prime_stamp(dir.path(), "99999999");

    let state = Stamp::status(dir.path()).expect("status ok");
    assert_eq!(
        state,
        State {
            held: false,
            pid: Some(99_999_999),
            stale: Some(true)
        }
    );
}

#[test]
fn stamp_status_malformed() {
    let dir = tempdir().expect("tempdir");
    prime_stamp(dir.path(), "not-a-pid\n");

    let state = Stamp::status(dir.path()).expect("status ok");
    assert_eq!(
        state,
        State {
            held: false,
            pid: None,
            stale: Some(true)
        }
    );
}
