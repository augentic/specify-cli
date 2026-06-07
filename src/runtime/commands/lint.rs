//! `specify lint *` dispatcher umbrella per the standards-layer contract.
//!
//! Composes the standards-layer pipeline that lives in `specify-standards`
//! (`lint::index::build` → `lint::eval::evaluate` →
//! `specify_diagnostics::render`) into the runtime CLI surface. Exit
//! codes route through `Exit::from(&Error)` per lint exit mapping and
//! [`crate::runtime::output`].

pub mod cli;
pub mod framework;
pub mod framework_tools;
pub mod project;
