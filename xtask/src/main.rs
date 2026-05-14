//! Workspace task runner: `gen-man` (`clap_mangen` roff pages).

use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod manpage;

#[derive(Parser)]
#[command(name = "xtask", about = "Specify CLI workspace task runner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render roff man pages for the `specify` binary and every
    /// (non-`help`) subcommand into `target/man/` via `clap_mangen`.
    /// One `.1` file per command, named `specify[-sub...].1`. The
    /// directory is gitignored; release tooling reads from there.
    GenMan,
}

const MAN_DIR: &str = "target/man";

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::GenMan => match manpage::render(Path::new(MAN_DIR)) {
            Ok(count) => {
                eprintln!("xtask gen-man: wrote {count} man page(s) to {MAN_DIR}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("xtask gen-man: {err}");
                ExitCode::from(2)
            }
        },
    }
}
