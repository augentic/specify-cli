//! Workspace task runner: `gen-man` (`clap_mangen` roff pages).

use std::path::PathBuf;
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
    /// (non-`help`) subcommand into `out_dir` via `clap_mangen`. One
    /// `.1` file per command, named `specify[-sub...].1`. The default
    /// `target/man/` is gitignored; release tooling reads from there.
    GenMan {
        /// Output directory for the rendered `.1` files. Created if
        /// missing.
        #[arg(long, default_value = "target/man")]
        out_dir: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::GenMan { out_dir } => match manpage::render(&out_dir) {
            Ok(count) => {
                eprintln!("xtask gen-man: wrote {count} man page(s) to {}", out_dir.display());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("xtask gen-man: {err}");
                ExitCode::from(2)
            }
        },
    }
}
