//! Command-world entry point for the `vectis-scaffold` WASI tool.

use std::process::ExitCode;

use clap::Parser;
use vectis_scaffold::{Args, render_envelope_json, run};

fn main() -> ExitCode {
    let args = Args::parse();
    let (json, code) = render_envelope_json(run(&args));
    println!("{json}");
    ExitCode::from(code)
}
