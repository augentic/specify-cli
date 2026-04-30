//! `specify` binary entry point.
//!
//! The binary is a thin dispatcher over the library: it parses CLI
//! arguments via `clap`, loads `.specify/project.yaml` (which transitively
//! enforces the `specify_version` floor), runs the subcommand, and maps
//! any error onto the exit-code contract below.
//!
//! # Exit codes — documented contract for skill authors
//!
//! - `0` ([`CliResult::Success`]): Success.
//! - `1` ([`CliResult::GenericFailure`]): Generic failure (I/O, parse,
//!   unknown).
//! - `2` ([`CliResult::ValidationFailed`]): Validation failed —
//!   `specify validate` returned a report whose `passed` flag is `false`.
//! - `3` ([`CliResult::VersionTooOld`]): The CLI binary is older than the
//!   `specify_version` floor in `.specify/project.yaml`.
//!
//! Error → exit code mapping:
//! - [`Error::SpecifyVersionTooOld`] → `3`.
//! - [`Error::Validation`] → `2`.
//! - Any other [`Error`] variant → `1`.
//! - A successful `Commands::Validate` where `report.passed == false` →
//!   `2` (even though no `Error` is produced).

#![allow(clippy::multiple_crate_versions)]

mod cli;
mod commands;
mod context;
pub(crate) mod output;

use std::process::ExitCode;

use clap::Parser;
use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    commands::run(cli).into()
}
