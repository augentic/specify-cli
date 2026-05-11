#![allow(
    clippy::multiple_crate_versions,
    reason = "The WASI tool runner pulls in Wasmtime/WASI transitive versions the workspace cannot unify yet."
)]

//! `specify` library crate.
//!
//! Hosts the command modules so workspace tooling (`xtask gen-man`,
//! future completions-from-xtask) can introspect the clap command tree
//! without spawning the binary. The `[[bin]]` target in
//! `src/main.rs` is a thin shim around [`run`].
//!
//! Exit-code contract for the dispatched [`run`] (defined by the
//! internal `Exit` enum in `output`):
//!
//! - `0` `Success`: Success.
//! - `1` `GenericFailure`: Generic failure (I/O, parse, tool
//!   resolver/runtime, unknown).
//! - `2` `ValidationFailed` or `ArgumentError`: validation failed or
//!   a post-parse argument-shape check failed.
//! - `3` `VersionTooOld`: The CLI binary is older than the
//!   `specify_version` floor in `.specify/project.yaml`.
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
