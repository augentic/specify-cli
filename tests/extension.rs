//! Integration tests for the `specify tool` surface (WASI tool dispatch,
//! tool-schema validation, and the contract tool). Shared helpers live in
//! [`common`].

mod common;

#[path = "tool/run.rs"]
mod run;

#[path = "tool/schema.rs"]
mod schema;

#[path = "tool/contract.rs"]
mod contract;
