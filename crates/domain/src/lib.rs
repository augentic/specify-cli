//! Specify domain — slice, change, spec, task, capability, registry,
//! config, merge, validate, init. See `docs/standards/architecture.md`
//! for the rationale.

pub mod capability;
pub mod change;
pub mod cmd;
pub mod config;
pub mod init;
pub mod merge;
pub mod registry;
pub mod serde_helpers;
pub mod serde_rfc3339;
pub mod slice;
pub mod spec;
pub mod task;
pub mod validate;
