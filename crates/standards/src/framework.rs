//! Framework authoring checks — the imperative `Check` pass behind
//! `specdev lint`. Predicates enforce framework authoring standards
//! (skill markdown shape, adapter manifests, brief discipline, prose
//! vocabulary, rule namespace ownership, …) by scanning the framework
//! repo through a [`context::Context`] and emitting canonical
//! [`specify_diagnostics::Diagnostic`]s via [`builder::framework_finding`];
//! [`check::run`] finalises each batch (project-relative locations,
//! fingerprints, sequential `FIND-NNNN` ids).
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
