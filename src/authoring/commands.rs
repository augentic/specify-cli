mod check;

use specify_authoring::exit::Exit;

use crate::authoring::cli::{Cli, Command};

pub fn run(cli: Cli) -> Exit {
    match cli.command {
        Command::Check { framework_root } => check::run(cli.format, framework_root),
    }
}
