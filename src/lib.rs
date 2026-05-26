//! Specify CLI library — hosts the `specrun` and `specdev` dispatch trees.

pub mod authoring;
pub mod runtime;
mod shared;

pub use runtime::run;
