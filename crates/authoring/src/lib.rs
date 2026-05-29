//! Framework authoring checks for `augentic/specify`.
//!
//! Library crate behind the `specdev lint` binary. The root `specify`
//! crate wires this checker into the `specdev` dispatcher; this crate
//! itself is `publish = false` and depends only on shared / standards
//! crates so framework-repo predicates never become part of the
//! `specrun` workflow dispatcher.
//!
//! ## Dependency overview
//!
//! - [`specify_error`] — shared error layering (payload-free
//!   `Error::Validation`) and the kebab-case wire discriminants.
//! - [`specify_schema`] — canonical embedded JSON Schemas
//!   (including `RULE_JSON_SCHEMA`) and the shared `jsonschema`
//!   plumbing (`compile_schema`, `validate_value`,
//!   `validate_serialisable`, `read_yaml_as_json`). The codex
//!   predicates consume the canonical schema through this crate
//!   directly — see [DECISIONS.md] for the long-form standards-layer rationale.
//! - [`specify_lints`] — the standards-layer crate providing the typed
//!   `Rule` DTO and the rules parser / resolver / finding
//!   validator that the codex-shape predicates depend on.
//!
//! [DECISIONS.md]: https://github.com/augentic/specify-cli/blob/main/DECISIONS.md

pub mod check;
pub mod context;
pub mod error;
pub mod exit;
pub mod finding;
pub mod helpers;
pub mod schema;

pub use context::Context;
pub use error::ToolingError;
pub use exit::Exit;
pub use finding::{Check, Finding, Location};
pub use helpers::{
    skill_body_lines, skill_frontmatter, strip_html_comments, under_symlink, walk_matching_files,
    walk_skill_files,
};
pub use schema::{SchemaError, SchemaId, ValidationError, validate_frontmatter, validate_value};
