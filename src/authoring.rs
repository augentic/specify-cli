//! `specdev` library surface — clap parse, dispatch, and exit mapping.

mod cli;
mod commands;
mod exit;

use std::process::ExitCode;

use clap::Parser;

/// Parse argv, dispatch the subcommand, and return the process exit
/// code. The `specdev` binary calls into this.
#[must_use]
pub fn run() -> ExitCode {
    let cli = cli::Cli::parse();
    ExitCode::from(commands::run(&cli).code())
}
