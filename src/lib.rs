#![allow(
    clippy::multiple_crate_versions,
    reason = "The RFC-15 tool runner pulls in Wasmtime/WASI transitive versions the workspace cannot unify yet."
)]

//! Top-level `specify` library crate. Hosts the local `config` and `init`
//! modules; everything else lives in the per-domain crates under
//! `crates/`.

pub mod config;
pub mod init;
