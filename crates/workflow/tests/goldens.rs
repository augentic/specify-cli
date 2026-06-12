//! Golden + fixture tests for the workflow contract: `validate_slice`
//! diagnostic goldens and the merge-engine goldens. Wire-schema
//! accept/reject fixtures live in the schema crate
//! (`crates/schema/tests/wire_fixtures.rs`). Shared helpers live in
//! [`common`].

mod common;

#[path = "goldens/slice_validate.rs"]
mod slice_validate;

#[path = "goldens/merge_engine.rs"]
mod merge_engine;
