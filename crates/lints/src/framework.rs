//! Framework authoring checks — the imperative `Check` pass behind
//! `specdev lint`.
//!
//! This module is the dissolved `specify-authoring` crate: the
//! imperative predicates that enforce framework authoring standards
//! (skill markdown shape, adapter manifests, brief discipline, prose
//! vocabulary, rule namespace ownership, …) live here, scanning the
//! framework repo through a [`context::Context`] rather than the
//! `WorkspaceModel` the declarative hint pass consumes.
//!
//! Unlike the lightweight `Finding` the predicates used to emit, every
//! predicate now builds the canonical [`crate::rules::Diagnostic`]
//! directly via [`builder::framework_finding`]. [`check::run`] finalises
//! the batch — rebasing locations to project-relative form, computing
//! fingerprints, and assigning sequential `FIND-NNNN` ids — so the
//! `specdev` binary's [`crate::lint::producer::DiagnosticProducer`]
//! bridge stays a thin wrapper.
//!
//! The framework `Check` trait survives (only its return type changed)
//! because the predicates need a `&Context`, which the
//! `DiagnosticProducer::produce(&WorkspaceModel, project_dir)` contract
//! does not provide.
//!
//! ## Lint posture
//!
//! The dissolved `specify-authoring` crate carried no `[lints]
//! workspace = true` stanza, so its predicates were never held to the
//! workspace's `missing_docs` / `missing_debug_implementations` /
//! `missing_copy_implementations` warnings. This relocation preserves
//! that posture verbatim with a module-scoped allow rather than churning
//! ~100 unit-struct derives and internal-doc comments onto code the
//! declarative burn-down will delete as each predicate migrates to
//! a `CORE-NNN` rule file.
#![allow(
    missing_docs,
    missing_debug_implementations,
    missing_copy_implementations,
    unused_qualifications,
    clippy::pedantic,
    clippy::nursery,
    clippy::string_add,
    clippy::iter_over_hash_type,
    clippy::map_err_ignore,
    clippy::unseparated_literal_suffix,
    reason = "relocated specify-authoring predicates retain their pre-merge lint posture: the dissolved authoring crate carried no `[lints] workspace = true` stanza, so the opt-in pedantic/nursery groups and these restriction cherry-picks never applied. The declarative burn-down deletes this code as each predicate migrates to a CORE-NNN rule file."
)]

pub mod builder;
pub mod check;
pub mod context;
pub mod error;
pub mod helpers;
pub mod schema;

pub use builder::{core_id_for, framework_finding, loc, snippet};
pub use check::Check;
pub use context::Context;
