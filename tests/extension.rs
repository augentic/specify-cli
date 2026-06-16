//! Integration tests for the `specify extension` surface (WASI extension
//! dispatch, extension-schema validation, and the contract extension).
//! Shared helpers live in [`common`].

mod common;

#[path = "extension/run.rs"]
mod run;

#[path = "extension/schema.rs"]
mod schema;

#[path = "extension/contract.rs"]
mod contract;
