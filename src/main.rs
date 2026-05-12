//! `specify` binary entry point. Thin shim over the `specify` library
//! crate (`src/lib.rs`); see `DECISIONS.md` for the exit-code contract.

use std::process::ExitCode;

fn main() -> ExitCode {
    specify::run()
}
