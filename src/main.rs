#![allow(
    clippy::multiple_crate_versions,
    reason = "The WASI tool runner pulls in Wasmtime/WASI transitive versions the workspace cannot unify yet."
)]

//! `specify` binary entry point. The dispatcher and command modules
//! live in the `specify` library crate (`src/lib.rs`); this shim
//! exists so `cargo install specify` produces an installable binary.
//! See the library crate docs for the exit-code contract.

use std::process::ExitCode;

fn main() -> ExitCode {
    specify::run()
}
