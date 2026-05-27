//! `specrun review *` dispatcher umbrella per RFC-32
//! §"`specrun review` (Phase 2 CLI)".
//!
//! Composes the standards-layer pipeline that lives in `specify-codex`
//! (`review::index::build` → `review::eval::evaluate` →
//! `review::diagnostics::render`) into the runtime CLI surface. Exit
//! codes route through `Exit::from(&Error)` per RFC-32 §D8 and
//! [`crate::runtime::output`].

pub mod cli;
pub mod run;
