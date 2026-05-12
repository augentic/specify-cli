//! Process invocation abstraction so tests can stub external CLIs.
//! Calls into `gh` / `git` go through [`CmdRunner`] so integration
//! tests can substitute a mock without faking out network or forge.

use std::io;
use std::process::{Command, Output};

/// Run a fully-prepared [`Command`] and return its [`Output`].
///
/// The trait is intentionally narrow — callers build the `Command`
/// (program, args, `current_dir`, etc.) and the runner is responsible
/// only for invoking it and harvesting the output. This keeps the
/// abstraction at the lowest possible level: no schema, no semantics,
/// no parsing.
pub trait CmdRunner {
    /// Execute `cmd` and return its `Output`. Mirrors
    /// [`Command::output`].
    ///
    /// # Errors
    ///
    /// Returns any I/O error encountered while spawning or waiting on
    /// the child process.
    fn run(&self, cmd: &mut Command) -> io::Result<Output>;
}

/// Default [`CmdRunner`] that actually spawns the child process.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealCmd;

impl CmdRunner for RealCmd {
    fn run(&self, cmd: &mut Command) -> io::Result<Output> {
        cmd.output()
    }
}
