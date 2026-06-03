//! Imperative diagnostic producer abstraction.
//!
//! The shared [`crate::lint::runner`] composes the declarative
//! hint-evaluator pass with zero or more imperative producers. A
//! producer is any source that, given the indexed [`WorkspaceModel`]
//! and the scan root, yields a batch of ready-to-render
//! [`Diagnostic`]s (project-relative locations, fingerprints already
//! stamped).
//!
//! The trait deliberately takes plain DTOs — never the runtime `Ctx`
//! or a `Plan` — so the standards layer stays free of workflow
//! lifecycle types. `specify lint framework` wraps the framework's imperative
//! `Check` predicates as one producer; `specify lint` passes none.
//!
//! The trait lives here, alongside the runner, rather than in the
//! neutral `specify-diagnostics` leaf because it references
//! [`WorkspaceModel`], which is a lint-surface concept. The diagnostic
//! currency it returns is the shared substrate; the plugin interface
//! that yields it is lint-runner machinery.

use std::path::Path;

use specify_diagnostics::Diagnostic;

use crate::lint::WorkspaceModel;

/// An imperative source of [`Diagnostic`]s composed by the shared
/// lint runner alongside the declarative hint pass.
pub trait DiagnosticProducer {
    /// Produce this source's findings against the indexed workspace.
    ///
    /// Implementations own location normalisation (project-relative
    /// paths) and fingerprint stamping so the runner can dedupe the
    /// combined set by fingerprint without re-canonicalising.
    fn produce(&self, model: &WorkspaceModel, project_dir: &Path) -> Vec<Diagnostic>;
}
