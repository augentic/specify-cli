//! Registry topology, shape validation, and workspace materialisation
//! for Specify.
//!
//! Owns parsing, helpers, and shape validators for `registry.yaml` (the
//! platform-level catalogue at the repo root) plus the local
//! `.specify/workspace/` materialisation surface (`sync`, `status`,
//! `push`, `merge`). Until RFC-13 chunk 2.2 the workspace code lived in
//! the binary's lib (`src/workspace.rs`, `src/workspace_merge.rs`); this
//! crate now owns it end to end.
//!
//! Dependency direction (post-RFC-13 §"Platform components are not
//! capabilities"):
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
//! cycle, [`workspace::run_workspace_push_impl`] and
//! [`merge::run_workspace_merge_impl`] take `initiative_name: &str`
//! rather than a `&Plan`. The binary flattens `&plan.name` at the
//! call-site.

pub mod gitignore;
pub mod merge;
pub mod registry;
pub mod validate;
pub mod workspace;

pub use gitignore::ensure_specify_gitignore_entries;
pub use registry::{ContractRoles, Registry, RegistryProject};
pub use validate::is_kebab_case;
