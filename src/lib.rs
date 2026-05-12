//! `specify` library crate. Hosts the command modules so workspace
//! tooling can introspect the clap command tree without spawning the
//! binary. See `DECISIONS.md` for the exit-code contract.

mod cli;
mod commands;
mod context;
pub(crate) mod output;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

/// Parse argv, dispatch the subcommand, and return the process exit
/// code. The `specify` binary calls into this.
#[must_use]
pub fn run() -> ExitCode {
    let cli = cli::Cli::parse();
    commands::run(cli).into()
}

/// Build the top-level [`clap::Command`] tree for the `specify`
/// binary without parsing argv. Used by workspace tooling that
/// inspects the command surface — currently `xtask gen-man`.
#[must_use]
pub fn command() -> clap::Command {
    cli::Cli::command()
}
