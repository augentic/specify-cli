//! Registry topology and shape validation for Specify.
//!
//! Owns parsing, helpers, and shape validators for `registry.yaml` (the
//! platform-level catalogue at the repo root). Workspace materialisation
//! (`.specify/workspace/`) lives in `src/workspace.rs` for now; it
//! migrates into this crate in RFC-13 chunk 2.2.
//!
//! Dependency direction (post-RFC-13 §"Platform components are not
//! capabilities"):
//!
//! ```text
//! specify-validate → specify-registry → specify-capability
//! ```
//!
//! `specify-capability` does **not** depend on this crate. Registry types
//! lived under `specify-capability` only because Phase 1 had no other
//! home for them.

pub mod registry;
pub mod validate;

pub use registry::{ContractRoles, Registry, RegistryProject};
pub use validate::is_kebab_case;
