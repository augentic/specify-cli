//! Specify CLI library — hosts the `specrun` and `specdev` dispatch trees.

pub mod authoring;
mod output;
pub mod runtime;

pub use output::Format;
pub use runtime::run;
