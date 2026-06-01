pub mod lint;

use crate::authoring::cli::{Cli, Command};
use crate::authoring::exit::Exit;

pub fn run(cli: &Cli) -> Exit {
    match &cli.command {
        Command::Lint(action) => lint::run(cli.format, action),
    }
}
