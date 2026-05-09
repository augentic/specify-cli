//! Workspace task runner.
//!
//! Currently exposes `standards-check`, the AST + regex enforcer for the
//! mechanical rules in [AGENTS.md#coding-standards]. CI runs this via
//! `cargo run -p xtask -- standards-check`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::StandardsCheck { root } => {
            let root = root.unwrap_or_else(repo_root);
            match standards::run(&root) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(1),
                Err(err) => {
                    eprintln!("xtask standards-check: {err}");
                    ExitCode::from(2)
                }
            }
        }
    }
}

fn repo_root() -> PathBuf {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .and_then(|out| {
            String::from_utf8(out.stdout)
                .ok()
                .map(|s| PathBuf::from(s.trim()))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}
