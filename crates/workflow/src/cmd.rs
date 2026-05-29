//! Command-runner abstraction so tests can substitute canned responses.
//!
//! [`CmdRunner`] is a borrowed callable so production sites pass
//! [`real_cmd`] (or `Command::output`) and test mocks pass closures
//! that capture per-test recording state.

use std::io;
use std::process::{Command, Output};

/// Borrowed callable that executes a fully-prepared [`Command`].
pub type CmdRunner<'a> = &'a dyn Fn(&mut Command) -> io::Result<Output>;

/// Default [`CmdRunner`] body that actually spawns the child process.
///
/// # Errors
///
/// Returns any I/O error encountered while spawning or waiting on the
/// child process.
pub fn real_cmd(cmd: &mut Command) -> io::Result<Output> {
    cmd.output()
}
