//! Registry topology, shape validation, and workspace materialisation
//! for Specify.
//!
//! Owns parsing, helpers, and shape validators for `registry.yaml` (the
//! platform-level catalogue at the repo root) plus the local
//! `.specify/workspace/` materialisation surface (`sync`, `status`,
//! `push`) plus read-only forge PR probes for finalization.
//!
//! Dependency direction (platform components are not capabilities):
//!
//! ```text
//! specify-validate → specify-slice → specify-registry → specify-capability
//! ```
//!
//! `specify-capability` does **not** depend on this crate. Registry
//! types lived under `specify-capability` only because Phase 1 had no
//! other home for them.
//!
//! The workspace layer is **upstream** of `specify-slice`: to avoid a
//! cycle, [`workspace::push_all`] takes the flattened plan
//! name rather than a `&Plan`. The binary passes `&plan.name` at the
//! call-site.

pub mod branch;
pub mod forge;
pub mod gitignore;
#[allow(
    clippy::module_inception,
    reason = "preserves the per-concern split inherited from the pre-collapse `specify-registry` crate; rename would cascade across many imports"
)]
pub mod registry;
pub mod validate;
pub mod workspace;

pub use gitignore::ensure_specify_gitignore_entries;
pub use registry::{ContractRoles, Registry, RegistryProject};
