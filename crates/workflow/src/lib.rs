//! Specify workflow — slice, change, adapter, registry, config, merge,
//! init lifecycle authority. The artifact model (spec, task, evidence,
//! discovery) lives in `specify-model`; artifact validation lives in
//! `specify-validate`. See `docs/standards/architecture.md` for the
//! rationale.

pub mod adapter;
pub mod change;
pub mod cmd;
pub mod config;
pub mod decisions;
pub mod design_system;
pub mod init;
pub mod journal;
pub mod merge;
pub mod migrate;
pub mod name;
pub mod plan_lock;
pub mod platform;
pub mod plugins;
pub mod registry;
pub mod schema;
pub mod slice;
pub mod upgrade;

pub use platform::{Platform, parse_platforms_csv};
