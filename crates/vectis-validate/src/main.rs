//! Command-world entry point for the `vectis-validate` WASI tool.

use std::process::ExitCode;

use clap::Parser;
use vectis_validate::{Args, render_envelope_json, run};

fn main() -> ExitCode {
    let args = Args::parse();
    let (json, code) = render_envelope_json(run(&args));
    println!("{json}");
    ExitCode::from(code)
}
