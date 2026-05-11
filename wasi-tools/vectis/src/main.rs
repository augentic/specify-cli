//! Command-world entry point for the `vectis` WASI tool.

use std::process::ExitCode;

use clap::Parser;
use specify_vectis::{Args, run};

fn main() -> ExitCode {
    let args = Args::parse();
    let (json, code) = run(&args);
    println!("{json}");
    ExitCode::from(code)
}
