//! Shared test helpers for `specify-workflow` integration tests.
//!
//! Centralises [`MockCmd`], a recorder that captures every invocation
//! and dispatches the response through a per-test closure. Pass it to
//! domain code as `&|cmd| mock.run(cmd)`.

#![expect(
    dead_code,
    reason = "shared test helpers; not every integration binary uses every helper"
)]
#![expect(
    clippy::unnecessary_wraps,
    reason = "mock dispatch closures share a Result<Output> signature for parity with real_cmd"
)]

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output};

/// One recorded invocation captured by [`MockCmd`].
#[derive(Debug, Clone)]
pub struct RecordedCall {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
}

type Handler = Box<dyn FnMut(&RecordedCall) -> io::Result<Output>>;

/// In-process command recorder that delegates dispatch to `handler`.
#[expect(
    clippy::partial_pub_fields,
    reason = "tests inspect `calls` directly; `handler` is an implementation detail of the closure dispatch"
)]
pub struct MockCmd {
    handler: RefCell<Handler>,
    pub calls: RefCell<Vec<RecordedCall>>,
}

impl MockCmd {
    /// Build a `MockCmd` from a dispatch closure.
    pub fn new<F>(handler: F) -> Self
    where
        F: FnMut(&RecordedCall) -> io::Result<Output> + 'static,
    {
        Self {
            handler: RefCell::new(Box::new(handler)),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// Record `cmd` and dispatch through the handler. Pass this method
    /// to domain code via `&|cmd| mock.run(cmd)`; the `&mut Command`
    /// expected by `CmdRunner` reborrows to `&Command` at the call.
    pub fn run(&self, cmd: &Command) -> io::Result<Output> {
        let recorded = RecordedCall {
            program: cmd.get_program().to_string_lossy().into_owned(),
            args: cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect(),
            current_dir: cmd.get_current_dir().map(PathBuf::from),
        };
        self.calls.borrow_mut().push(recorded.clone());
        (self.handler.borrow_mut())(&recorded)
    }
}

/// Produce a successful [`Output`] with `stdout` (no stderr).
pub fn ok_stdout(stdout: &str) -> io::Result<Output> {
    Ok(Output {
        status: success_status(),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    })
}

/// Produce a successful [`Output`] with no stdout or stderr.
pub fn ok_empty() -> io::Result<Output> {
    ok_stdout("")
}

/// Produce an [`Output`] whose exit status is failure with `stderr`.
pub fn fail_stderr(stderr: &str) -> io::Result<Output> {
    Ok(Output {
        status: failure_status(),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

#[cfg(unix)]
fn success_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[cfg(unix)]
fn failure_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(1 << 8)
}

// `copy_dir` (and the git helper trio) come from the workspace-shared
// helper file; see `tests/common/fs_git.rs` at the repo root. It is
// re-exposed as a thin wrapper (rather than a `pub use`) so binaries
// that never stage a fixture see it as `dead_code` — covered by the
// module-level expectation — instead of an unused `pub use`.
#[path = "../../../../tests/common/fs_git.rs"]
mod fs_git;
pub fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    fs_git::copy_dir(src, dst);
}
