use std::path::PathBuf;

use tempfile::TempDir;

use super::{LockProbe, probe};

fn lock_path(dir: &TempDir) -> PathBuf {
    let specify = dir.path().join(".specify");
    std::fs::create_dir_all(&specify).expect("mkdir .specify");
    specify.join("plan.lock")
}

#[test]
fn missing_lockfile_is_unheld() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(".specify").join("plan.lock");
    assert_eq!(probe(&path).expect("probe"), LockProbe::Unheld);
}

#[test]
fn unlocked_file_is_unheld() {
    let dir = TempDir::new().expect("tempdir");
    let path = lock_path(&dir);
    std::fs::write(&path, "pid=0\n").expect("seed lockfile");
    assert_eq!(probe(&path).expect("probe"), LockProbe::Unheld);
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};

    use specify_error::Error;

    use super::*;
    use crate::config::Layout;
    use crate::plan_lock::require_held;

    /// Hold a `flock`-family exclusive lock for the guard's lifetime —
    /// the same lock family as both blessed plan-lock.md snippets
    /// (`flock(1)` and Python's `fcntl.flock`). Dropping the guard
    /// closes the descriptor, which releases the lock.
    struct FlockGuard {
        _file: File,
    }

    impl FlockGuard {
        fn acquire(path: &Path) -> Self {
            let file = File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
                .expect("open lockfile");
            file.lock().expect("test flock acquire failed");
            Self { _file: file }
        }
    }

    #[test]
    fn flock_holder_is_held() {
        let dir = TempDir::new().expect("tempdir");
        let path = lock_path(&dir);
        let guard = FlockGuard::acquire(&path);
        assert_eq!(probe(&path).expect("probe"), LockProbe::Held);
        drop(guard);
        assert_eq!(probe(&path).expect("probe"), LockProbe::Unheld, "release must unlock");
    }

    #[test]
    fn require_held_refuses_unlocked() {
        let dir = TempDir::new().expect("tempdir");
        let _path = lock_path(&dir);
        let err = require_held(Layout::new(dir.path())).expect_err("must refuse");
        match err {
            Error::Validation { code, .. } => assert_eq!(code, "plan-lock-not-held"),
            other => panic!("expected Error::Validation, got {other:?}"),
        }
    }

    #[test]
    fn require_held_passes_under_lock() {
        let dir = TempDir::new().expect("tempdir");
        let path = lock_path(&dir);
        let _guard = FlockGuard::acquire(&path);
        require_held(Layout::new(dir.path())).expect("held lock must pass");
    }

    /// Spawned python3 holding an `fcntl(2)` record lock (`lockf`) —
    /// the lock family zsh's `zsystem flock` uses. On Linux this is
    /// invisible to `flock(2)`, so it exercises the `F_GETLK` arm of
    /// the probe; the holder must be a separate process because a
    /// process's own record locks never conflict with its `F_GETLK`.
    struct PythonFcntlHolder {
        child: Child,
    }

    impl PythonFcntlHolder {
        /// `None` when python3 is unavailable on this machine.
        fn spawn(path: &Path) -> Option<Self> {
            let script = "\
import fcntl, sys
fd = open(sys.argv[1], 'a+')
fcntl.lockf(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
print('locked', flush=True)
sys.stdin.readline()
";
            let mut child = Command::new("python3")
                .arg("-c")
                .arg(script)
                .arg(path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            // Handshake: the lock is held once the child prints.
            let stdout = child.stdout.take().expect("piped stdout");
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).ok()?;
            if line.trim() != "locked" {
                drop(child.kill());
                return None;
            }
            Some(Self { child })
        }
    }

    impl Drop for PythonFcntlHolder {
        fn drop(&mut self) {
            if let Some(stdin) = self.child.stdin.take() {
                drop(stdin);
            }
            drop(self.child.kill());
            drop(self.child.wait());
        }
    }

    #[test]
    fn fcntl_holder_is_held() {
        let dir = TempDir::new().expect("tempdir");
        let path = lock_path(&dir);
        std::fs::write(&path, "").expect("seed lockfile");
        let Some(holder) = PythonFcntlHolder::spawn(&path) else {
            eprintln!("skipping: python3 unavailable");
            return;
        };
        assert_eq!(
            probe(&path).expect("probe"),
            LockProbe::Held,
            "F_GETLK must see the cross-process record lock"
        );
        drop(holder);
        assert_eq!(probe(&path).expect("probe"), LockProbe::Unheld, "exit must unlock");
    }

    /// The probe itself must not leave the lock held (the try-acquire
    /// arm releases on success): two probes in a row both say unheld,
    /// and a snippet-style acquire still succeeds afterwards.
    #[test]
    fn probe_leaves_lock_free() {
        let dir = TempDir::new().expect("tempdir");
        let path = lock_path(&dir);
        std::fs::write(&path, "").expect("seed lockfile");
        assert_eq!(probe(&path).expect("first probe"), LockProbe::Unheld);
        assert_eq!(probe(&path).expect("second probe"), LockProbe::Unheld);
        let _guard = FlockGuard::acquire(&path);
        assert_eq!(probe(&path).expect("probe under lock"), LockProbe::Held);
    }
}
