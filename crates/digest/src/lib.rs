//! SHA-256 digest encoding shared across Specify workspace crates.
//!
//! Keeps digest helpers out of domain crates so dependents avoid
//! pulling unrelated dependency graphs (for example `specify-standards`
//! must not depend on `specify-tool` / Wasmtime for a hex digest).

pub mod hash;

pub use hash::{sha256_hex, sha256_output_hex};
