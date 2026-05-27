//! Specify codex — RFC-28 codex rule parser, resolver, and review
//! finding validators.
//!
//! Per RFC-32 §"Library layout", this crate is a standards-layer
//! sibling of `specify-domain`: it carries every codex DTO, the CH-11
//! frontmatter parser, the CH-12/13/14 resolver pipeline, the CH-15
//! fingerprint algorithm, and the CH-16 finding validators. The
//! separation is enforced by the type system — `specify-codex` MUST
//! NOT depend on `specify-domain`, and `specify-domain` MUST NOT
//! depend on `specify-codex`.
//!
//! The internal module shape preserves the RFC-28 vs RFC-32 split:
//! the [`rules`] umbrella wraps the codex-rule, resolver, fingerprint
//! and finding modules so paths like `specify_codex::rules::parse`
//! stay stable for downstream consumers; the top-level re-export
//! mirrors the historical `specify_domain::codex::*` surface so call
//! sites only change their crate prefix.

pub mod review;
pub mod rules;

pub use rules::*;
