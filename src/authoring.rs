//! `specdev` library surface — clap parse and dispatch.
//!
//! Exit mapping reuses the runtime's single `Exit::from(&Error)` table
//! (`crate::runtime::output`) so both binaries share one source of
//! truth; `specdev` adds no bespoke exit codes.

mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;

use crate::runtime::output::{self, Exit};

/// Parse argv, dispatch the subcommand, and return the process exit code.
///
/// The `specdev` binary calls into this. Handlers return `Result<()>`;
/// a terminal error is rendered on stderr and mapped to its exit code
/// by the shared runtime `output::report`, exactly as `specrun` does.
#[must_use]
pub fn run() -> ExitCode {
    let cli = cli::Cli::parse();
    let exit = match commands::run(&cli) {
        Ok(()) => Exit::Success,
        Err(err) => output::report(cli.format, &err),
    };
    ExitCode::from(exit)
}
