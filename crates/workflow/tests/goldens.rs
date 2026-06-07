//! Golden + fixture tests for the workflow contract: `validate_slice`
//! diagnostic goldens, the bundled JSON Schemas, and the Python-reference
//! parity outputs. Shared helpers live in [`common`].

mod common;

#[path = "goldens/slice_validate.rs"]
mod slice_validate;

#[path = "goldens/schemas.rs"]
mod schemas;

#[path = "goldens/parity.rs"]
mod parity;
