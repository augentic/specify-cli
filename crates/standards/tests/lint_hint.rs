//! Integration tests for the per-kind deterministic hint evaluators.
//!
//! Each themed submodule exercises one `kind:` arm of the generic lint
//! dispatcher (Road A) plus the Road B `kind: tool` resolver. Shared
//! fixtures and runners live in [`eval_support`].

mod eval_support;

#[path = "lint_hint/cardinality.rs"]
mod cardinality;
#[path = "lint_hint/constant_eq.rs"]
mod constant_eq;
#[path = "lint_hint/content_digest_eq.rs"]
mod content_digest_eq;
#[path = "lint_hint/cross_reference.rs"]
mod cross_reference;
#[path = "lint_hint/fenced_block.rs"]
mod fenced_block;
#[path = "lint_hint/field_grammar.rs"]
mod field_grammar;
#[path = "lint_hint/ignore_directive_pass.rs"]
mod ignore_directive_pass;
#[path = "lint_hint/path_pattern.rs"]
mod path_pattern;
#[path = "lint_hint/presence.rs"]
mod presence;
#[path = "lint_hint/reference_resolves.rs"]
mod reference_resolves;
#[path = "lint_hint/regex.rs"]
mod regex;
#[path = "lint_hint/schema.rs"]
mod schema;
#[path = "lint_hint/set_coverage.rs"]
mod set_coverage;
#[path = "lint_hint/set_eq.rs"]
mod set_eq;
#[path = "lint_hint/tool.rs"]
mod tool;
#[path = "lint_hint/unique.rs"]
mod unique;
