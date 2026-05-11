use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};

use specify_error::Error;
use tempfile::tempdir;

use super::*;

fn read_lock_pid(dir: &Path) -> String {
    fs::read_to_string(dir.join(".specify").join("plan.lock")).expect("read lockfile")
}

#[test]
fn acquire_and_release() {
    let dir = tempdir().expect("tempdir");
    let guard = Guard::acquire(dir.path()).expect("acquire ok");

    let lock_path = dir.path().join(".specify").join("plan.lock");
    assert!(lock_path.exists(), "lockfile should exist while guard is held");
    assert_eq!(read_lock_pid(dir.path()).trim(), std::process::id().to_string());
    assert_eq!(guard.pid(), std::process::id());
    assert_eq!(guard.reclaimed_stale_pid(), None);

    drop(guard);
    assert!(!lock_path.exists(), "lockfile should be removed on drop");
}

#[test]
fn second_acquire_is_busy() {
    let dir = tempdir().expect("tempdir");
    let _first = Guard::acquire_with_liveness_check(dir.path(), |_| true).expect("first ok");

    let err =
        Guard::acquire_with_liveness_check(dir.path(), |_| true).expect_err("second should fail");
    match err {
        Error::DriverBusy { pid } => assert_eq!(pid, std::process::id()),
        other => panic!("expected DriverBusy, got {other:?}"),
    }
}

#[test]
fn stale_lock_reclaimed() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "99999").expect("prime stale");

    let guard = Guard::acquire_with_liveness_check(dir.path(), |_| false).expect("reclaim ok");
    assert_eq!(guard.reclaimed_stale_pid(), Some(99999));
    assert_eq!(read_lock_pid(dir.path()).trim(), std::process::id().to_string());
}

#[test]
fn malformed_pid_reclaimed() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "not-a-number\n")
        .expect("prime malformed");

    let guard = Guard::acquire_with_liveness_check(dir.path(), |_| true).expect("reclaim ok");
    assert_eq!(
        guard.reclaimed_stale_pid(),
        None,
        "malformed contents carry no prior PID to report"
    );
    assert_eq!(read_lock_pid(dir.path()).trim(), std::process::id().to_string());
}

#[test]
fn drop_removes_on_panic() {
    let dir = tempdir().expect("tempdir");
    let dir_path = dir.path().to_path_buf();
    let lock_path = dir_path.join(".specify").join("plan.lock");

    let result = std::panic::catch_unwind(|| {
        let _guard = Guard::acquire(&dir_path).expect("acquire ok");
        panic!("simulated failure while holding lock");
    });
    assert!(result.is_err(), "inner closure should have panicked");
    assert!(!lock_path.exists(), "lockfile should be cleaned on unwind");
}

#[test]
fn reclaim_diagnostic() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "99999").expect("prime stale");

    let guard = Guard::acquire_with_liveness_check(dir.path(), |_| false).expect("reclaim ok");

    assert_eq!(guard.reclaimed_stale_pid(), Some(99999));
}

// ------------------------------------------------------------------
// Stamp (PID-only stamp used by the CLI lock verbs)
// ------------------------------------------------------------------

#[test]
fn stamp_acquire_release() {
    let dir = tempdir().expect("tempdir");
    let acquired =
        Stamp::acquire_with_liveness_check(dir.path(), 4242, |_| true).expect("acquire ok");
    assert_eq!(acquired.pid, 4242);
    assert_eq!(acquired.reclaimed_stale_pid, None);
    assert!(!acquired.already_held);
    assert_eq!(read_lock_pid(dir.path()).trim(), "4242");

    let released = Stamp::release(dir.path(), 4242).expect("release ok");
    assert_eq!(released, PlanLockReleased::Removed { pid: 4242 });
    assert!(!dir.path().join(".specify").join("plan.lock").exists());
}

#[test]
fn stamp_reacquire_idempotent() {
    let dir = tempdir().expect("tempdir");
    Stamp::acquire_with_liveness_check(dir.path(), 1234, |_| true).expect("first");
    let again =
        Stamp::acquire_with_liveness_check(dir.path(), 1234, |_| true).expect("reacquire ok");
    assert!(again.already_held, "same-PID re-stamp must report already_held");
    assert_eq!(again.reclaimed_stale_pid, None);
}

#[test]
fn stamp_acquire_busy() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "7777").expect("prime");

    let err = Stamp::acquire_with_liveness_check(dir.path(), 4242, |_| true)
        .expect_err("expected DriverBusy");
    assert!(matches!(err, Error::DriverBusy { pid: 7777 }));
    // Contents unchanged — we never clobbered the live holder.
    assert_eq!(read_lock_pid(dir.path()).trim(), "7777");
}

#[test]
fn stamp_reclaims_stale() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "99999").expect("prime stale");

    let acquired =
        Stamp::acquire_with_liveness_check(dir.path(), 4242, |_| false).expect("reclaim ok");
    assert_eq!(acquired.reclaimed_stale_pid, Some(99999));
    assert_eq!(read_lock_pid(dir.path()).trim(), "4242");
}

#[test]
fn stamp_release_absent() {
    let dir = tempdir().expect("tempdir");
    let released = Stamp::release(dir.path(), 4242).expect("release ok");
    assert_eq!(released, PlanLockReleased::WasAbsent);
}

#[test]
fn stamp_release_refuses_other() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "7777").expect("prime");

    let released = Stamp::release(dir.path(), 4242).expect("release ok");
    assert_eq!(released, PlanLockReleased::HeldByOther { pid: Some(7777) });
    // File still there — we refused to clobber.
    assert_eq!(read_lock_pid(dir.path()).trim(), "7777");
}

#[test]
fn stamp_status_absent() {
    let dir = tempdir().expect("tempdir");
    let state = Stamp::status_with_liveness_check(dir.path(), |_| true).expect("status ok");
    assert_eq!(
        state,
        PlanLockState {
            held: false,
            pid: None,
            stale: None
        }
    );
}

#[test]
fn stamp_status_held() {
    let dir = tempdir().expect("tempdir");
    Stamp::acquire_with_liveness_check(dir.path(), 4242, |_| true).expect("acquire");

    let state = Stamp::status_with_liveness_check(dir.path(), |_| true).expect("status ok");
    assert_eq!(
        state,
        PlanLockState {
            held: true,
            pid: Some(4242),
            stale: Some(false)
        }
    );
}

#[test]
fn stamp_status_stale() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "99999").expect("prime stale");

    let state = Stamp::status_with_liveness_check(dir.path(), |_| false).expect("status ok");
    assert_eq!(
        state,
        PlanLockState {
            held: false,
            pid: Some(99999),
            stale: Some(true)
        }
    );
}

#[test]
fn stamp_status_malformed() {
    let dir = tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join(".specify")).expect("mkdir");
    fs::write(dir.path().join(".specify").join("plan.lock"), "not-a-pid\n").expect("prime");

    let state = Stamp::status_with_liveness_check(dir.path(), |_| true).expect("status ok");
    assert_eq!(
        state,
        PlanLockState {
            held: false,
            pid: None,
            stale: Some(true)
        }
    );
}

#[test]
fn cross_thread_acquire_is_busy() {
    // Cross-thread acquisition is verified via the liveness
    // override rather than raw flock semantics, which per the
    // module-level doc comment we consider belt-plus-PID-file.
    let dir = tempdir().expect("tempdir");
    let dir_path = dir.path().to_path_buf();

    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    let holder_dir = dir_path.clone();
    let holder = thread::spawn(move || {
        let guard =
            Guard::acquire_with_liveness_check(&holder_dir, |_| true).expect("holder acquire ok");
        started_tx.send(()).expect("notify started");
        release_rx.recv().expect("await release signal");
        drop(guard);
    });

    started_rx.recv().expect("holder started");

    let err = Guard::acquire_with_liveness_check(&dir_path, |_| true)
        .expect_err("contender should see DriverBusy");
    assert!(matches!(err, Error::DriverBusy { .. }));

    release_tx.send(()).expect("release holder");
    holder.join().expect("holder joined");

    // After release, a fresh acquire should succeed.
    thread::sleep(Duration::from_millis(10));
    let _after =
        Guard::acquire_with_liveness_check(&dir_path, |_| true).expect("post-release acquire ok");
}
