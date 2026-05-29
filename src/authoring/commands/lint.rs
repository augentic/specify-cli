//! `specdev lint` — framework convergence verb per RFC-34 §F2.
//!
//! Composes two enforcement passes against the framework repo:
//!
//! 1. The imperative `Check` predicates in
//!    [`specify_authoring::check`] (today's `make check` surface).
//! 2. The declarative deterministic-hint interpreter in
//!    [`specify_lints::lint`] driven by `CORE-*` / `UNI-*` rules
//!    under the framework's own codex tree.
//!
//! Both passes emit [`specify_lints::Diagnostic`]; the dispatcher
//! folds them into a single [`specify_lints::lint::diagnostics::DiagnosticReport`]
//! envelope, deduplicates by fingerprint per RFC-34 §F5, mints the
//! reserved-hint diagnostics summary, and renders through the four
//! formatters in [`specify_lints::lint::diagnostics`]. One
//! `lint-completed` journal event lands per run (RFC-34 §F7).

mod cli;
mod run;

pub use cli::LintAction;
pub use run::run;
