//! `specrun lint *` dispatcher umbrella per the standards-layer contract
//! §"`specrun lint` (Phase 2 CLI)".
//!
//! Composes the standards-layer pipeline that lives in `specify-lints`
//! (`lint::index::build` → `lint::eval::evaluate` →
//! `lint::diagnostics::render`) into the runtime CLI surface. Exit
//! codes route through `Exit::from(&Error)` per lint exit mapping and
//! [`crate::runtime::output`].

pub mod cli;
pub mod run;
