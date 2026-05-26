//! `specdev` binary entry point. Thin shim over the authoring library
//! module.

use std::process::ExitCode;

fn main() -> ExitCode {
    specify::authoring::run()
}
