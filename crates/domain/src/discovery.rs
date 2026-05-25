//! Discovery surface — `## Candidate inventory` blocks in
//! `discovery.md` and the candidate shape source adapters emit at
//! `enumerate` time. Validated against
//! `schemas/discovery/candidate.schema.json`.
//!
//! The whole-document model lives in [`document`] (workflow §D6); it
//! parses `discovery.md`, exposes [`Discovery::resolve_candidate`]
//! for the `--sources <key>=<id-or-alias>` rewrite path, and gates
//! the cross-candidate `id` ↔ `aliases[]` collision check shared
//! between `specrun plan amend --add-alias` and
//! `specrun slice validate`.

pub mod candidate;
pub mod document;

pub use candidate::{AliasCollision, Candidate, CandidateAliases};
pub use document::{Discovery, DiscoveryAliasCollision, ResolveError as DiscoveryResolveError};
