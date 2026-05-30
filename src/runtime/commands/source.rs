//! `specrun source {resolve, cache, preview, survey, extract}` —
//! source adapter operations and the extraction cache fingerprint
//! contract cache surface.
//!
//! `resolve` shares the run-side dispatch with the target axis on the
//! unified `commands::resolve_plugin` helper (it is byte-identical to
//! the target-axis path apart from the `@version` peel). `cache`
//! owns the extraction cache fingerprint contract fingerprint lookup / write / index reader path
//! and lives in its own module under [`cache`]. `preview` is the
//! `specrun source preview` contract workflow-free source adapter scaffolding verb.
//! [`prep`] is the internal prep seam (adapter resolution, brief
//! directory, the four-root sandbox layout, and `evidence/`
//! scaffolding) shared by `preview` today and the RFC-29a C6/C7
//! `survey` / `extract` runners.

pub mod cache;
pub mod cli;
pub mod extract;
pub mod prep;
pub mod preview;
pub mod survey;
