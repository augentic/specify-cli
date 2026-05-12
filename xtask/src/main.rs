//! Workspace task runner.
//!
//! Hosts `standards-check` (mechanical coding-standards enforcer),
//! `gen-man` (`clap_mangen` roff pages), and `gen-completions`
//! (`clap_complete` shell scripts).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod completions;
mod manpage;
mod standards;

#[derive(Parser)]
#[command(name = "xtask", about = "Specify CLI workspace task runner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Mechanical enforcement of coding standards. Reads per-file
    /// baselines from `scripts/standards-allowlist.toml`; fails if a
    /// live count exceeds its baseline.
    StandardsCheck {
        /// Repository root (defaults to git toplevel).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Rewrite `scripts/standards-allowlist.toml` so every per-file
        /// baseline matches today's actual count. Use after a
        /// migration shrinks a file's count to lock in the gain.
        #[arg(long, conflicts_with = "check_tightenable")]
        tighten: bool,
        /// Fail (exit 1) when any per-file baseline could be tightened.
        /// CI runs this so unrelated PRs cannot mask incidental progress.
        #[arg(long)]
        check_tightenable: bool,
    },
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
    /// Render `clap_complete` shell-completion scripts for the
    /// `specify` binary into `out_dir/<shell>/specify.<ext>` for every
    /// `clap_complete::Shell` value. The default `target/completions/`
    /// is gitignored; release tooling reads from there.
    GenCompletions {
        /// Output directory for the rendered completion scripts.
        /// Created (with one subdirectory per shell) if missing.
        #[arg(long, default_value = "target/completions")]
        out_dir: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::StandardsCheck {
            root,
            tighten,
            check_tightenable,
        } => {
            let root = root.unwrap_or_else(repo_root);
            let mode = if tighten {
                standards::Mode::Tighten
            } else if check_tightenable {
                standards::Mode::CheckTightenable
            } else {
                standards::Mode::Check
            };
            match standards::run(&root, mode) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(1),
                Err(err) => {
                    eprintln!("xtask standards-check: {err}");
                    ExitCode::from(2)
                }
            }
        }
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
        Command::GenCompletions { out_dir } => match completions::render(&out_dir) {
            Ok(count) => {
                eprintln!(
                    "xtask gen-completions: wrote {count} completion script(s) to {}",
                    out_dir.display()
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("xtask gen-completions: {err}");
                ExitCode::from(2)
            }
        },
    }
}

fn repo_root() -> PathBuf {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok().map(|s| PathBuf::from(s.trim())))
        .unwrap_or_else(|| PathBuf::from("."))
}
