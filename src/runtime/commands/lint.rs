//! `specrun lint *` dispatcher umbrella per RFC-32
//! §"`specrun lint` (Phase 2 CLI)".
//!
//! Composes the standards-layer pipeline that lives in `specify-codex`
//! (`lint::index::build` → `lint::eval::evaluate` →
//! `lint::diagnostics::render`) into the runtime CLI surface. Exit
//! codes route through `Exit::from(&Error)` per RFC-32 §D8 and
//! [`crate::runtime::output`].

pub mod cli;
pub mod run;
