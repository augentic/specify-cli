//! `specify source {resolve, cache}` — RFC-25 source adapter
//! operations and the RFC-27 §D8 cache surface.
//!
//! `resolve` shares the run-side dispatch with the target axis on the
//! unified `commands::resolve_plugin` helper (it is byte-identical to
//! the target-axis path apart from the `@version` peel). `cache`
//! owns the RFC-27 §D8 fingerprint lookup / write / index reader path
//! and lives in its own module under [`cache`].

pub mod cache;
pub mod cli;
