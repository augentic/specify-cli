//! Framework authoring substrate.
//! `specify lint framework` runs entirely through declarative hints and
//! referenced WASI tools, so no imperative `Check` predicate is wired in
//! as a producer. The surviving `Check` impls are the repo-local
//! Rust-quality predicates ([`check::RustTestNaming`],
//! [`check::RustSourceQuality`]), which scan the framework repo through a
//! [`context::Context`] and emit canonical
//! [`specify_diagnostics::Diagnostic`]s via [`builder::framework_finding`];
//! the `check` finalize pass finalises each batch (project-relative
//! locations, fingerprints, sequential `FIND-NNNN` ids).
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
