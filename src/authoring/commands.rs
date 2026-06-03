pub mod lint;

use specify_error::Result;

use crate::authoring::cli::{Cli, Command};

/// Dispatch the parsed `specdev` subcommand. Returns `Result<()>` so
/// the binary surface maps the terminal error through the shared
/// runtime exit table — there is no `specdev`-local exit enum.
///
/// # Errors
///
/// Propagates the handler error for the dispatched subcommand.
pub fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Lint(action) => lint::run(cli.format, action),
    }
}
