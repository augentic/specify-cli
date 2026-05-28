//! `specrun source {resolve, cache, preview}` — source adapter
//! operations and the extraction cache fingerprint contract cache surface.
//!
//! `resolve` shares the run-side dispatch with the target axis on the
//! unified `commands::resolve_plugin` helper (it is byte-identical to
//! the target-axis path apart from the `@version` peel). `cache`
//! owns the extraction cache fingerprint contract fingerprint lookup / write / index reader path
//! and lives in its own module under [`cache`]. `preview` is the
//! `specrun source preview` contract workflow-free source adapter scaffolding verb.

pub mod cache;
pub mod cli;
pub mod preview;
