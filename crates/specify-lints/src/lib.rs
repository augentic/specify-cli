//! Specify lints (standards layer) — rule parser, resolver, and review
//! finding validators.
//!
//! Per the standards-layer dependency invariant, this crate is a standards-layer
//! sibling of `specify-domain`: it carries every rules/lint DTO, the CH-11
//! frontmatter parser, the CH-12/13/14 resolver pipeline, the CH-15
//! fingerprint algorithm, and the CH-16 finding validators. The
//! separation is enforced by the type system — `specify-lints` MUST
//! NOT depend on `specify-domain`, and `specify-domain` MUST NOT
//! depend on `specify-lints`.
//!
//! **Crate name vs modules:** `specify-lints` is the whole standards layer
//! (policy resolution plus the `specrun lint` scanner). The [`rules`]
//! module owns parsing and `specrun rules export`; [`lint`] owns indexing
//! and hint evaluation — not a separate `specify-rules` crate.
//!
//! The internal module shape preserves the rules-vs-lint split:
//! the [`rules`] umbrella wraps the rule, resolver, fingerprint
//! and finding modules so paths like `specify_lints::rules::parse`
//! stay stable for downstream consumers; the top-level re-export
//! mirrors the historical `specify_domain::rules::*` surface so call
//! sites only change their crate prefix.

pub mod lint;
pub mod rules;

pub use rules::*;
