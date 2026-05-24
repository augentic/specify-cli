//! `specify` library crate. Hosts the command modules behind the
//! binary entry point. See `DECISIONS.md` for the exit-code contract.

mod cli;
mod commands;
mod context;
pub(crate) mod output;

use std::process::ExitCode;

use clap::Parser;

/// Parse argv, dispatch the subcommand, and return the process exit
/// code. The `specify` binary calls into this.
#[must_use]
pub fn run() -> ExitCode {
    let cli = cli::Cli::parse();
    commands::run(cli).into()
}
