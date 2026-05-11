#![allow(
    clippy::multiple_crate_versions,
    reason = "The WASI tool runner pulls in Wasmtime/WASI transitive versions the workspace cannot unify yet."
)]

//! `specify` binary entry point.
//!
//! The binary is a thin dispatcher over workspace crates: it parses CLI
//! arguments via `clap`, loads `.specify/project.yaml` for project-aware
//! commands (which transitively enforces the `specify_version` floor), runs
//! the subcommand, and maps any error onto the exit-code contract below.
//!
//! # Exit codes — documented contract for skill authors
//!
//! - `0` ([`crate::output::CliResult::Success`]): Success.
//! - `1` ([`crate::output::CliResult::GenericFailure`]): Generic failure
//!   (I/O, parse, tool resolver/runtime, unknown).
//! - `2` ([`crate::output::CliResult::ValidationFailed`] or
//!   [`crate::output::CliResult::ArgumentError`]): validation failed or a
//!   post-parse argument-shape check failed.
//! - `3` ([`crate::output::CliResult::VersionTooOld`]): The CLI binary
//!   is older than the `specify_version` floor in
//!   `.specify/project.yaml`.
//!
//! Error → exit code mapping:
//! - [`specify_error::Error::CliTooOld`] → `3`.
//! - [`specify_error::Error::Validation`],
//!   [`specify_error::Error::ToolDenied`], and
//!   [`specify_error::Error::ToolNotDeclared`] → `2`.
//! - [`specify_error::Error::Argument`] → `2`.
//! - Any other [`specify_error::Error`] variant → `1`.

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
