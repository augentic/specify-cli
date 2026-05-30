//! `specdev lint` — framework convergence verb.
//!
//! Composes two enforcement passes against the framework repo:
//!
//! 1. The imperative `Check` predicates in
//!    [`specify_standards::framework::check`] (today's `make check` surface).
//! 2. The declarative deterministic-hint interpreter in
//!    [`specify_standards::lint`] driven by `CORE-*` / `UNI-*` rules
//!    under the framework's own codex tree.
//!
//! Both passes emit [`specify_diagnostics::Diagnostic`]; the dispatcher
//! folds them into a single [`specify_diagnostics::DiagnosticReport`]
//! envelope, deduplicates by fingerprint, mints the
//! reserved-hint diagnostics summary, and renders through the four
//! formatters in [`specify_diagnostics`]. One
//! `lint-completed` journal event lands per run.

mod cli;
mod run;

pub use cli::LintAction;
pub use run::run;
