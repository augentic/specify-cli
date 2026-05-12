//! Registry topology, shape validation, and workspace materialisation
//! for Specify.

pub mod branch;
pub mod catalog;
pub mod forge;
pub mod gitignore;
pub mod validate;
pub mod workspace;

pub use catalog::{ContractRoles, Registry, RegistryProject};
pub use gitignore::ensure_specify_gitignore_entries;
