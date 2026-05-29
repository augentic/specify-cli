pub mod lint;

use specify_authoring::exit::Exit;

use crate::authoring::cli::{Cli, Command};

pub fn run(cli: &Cli) -> Exit {
    match &cli.command {
        Command::Lint(action) => lint::run(cli.format, action),
    }
}
