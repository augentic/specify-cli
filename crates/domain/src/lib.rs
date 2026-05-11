//! Specify domain — slice, change, spec, task, capability, registry,
//! config, merge, validate, init. Single crate to keep the dependency
//! graph shallow; submodules preserve the original boundaries.
//!
//! See `docs/standards/architecture.md` for the rationale.

#![allow(
    clippy::multiple_crate_versions,
    reason = "ProjectConfig re-exports `specify_tool::Tool`, which transitively pulls in Wasmtime/WASI duplicate versions. See DECISIONS.md."
)]

#[macro_use]
mod macros;

pub mod spec;
pub mod task;
pub mod capability;
pub mod registry;
pub mod slice;
pub mod config;
pub mod merge;
pub mod validate;
pub mod change;
pub mod init;
pub mod serde_helpers;
pub mod serde_rfc3339;
