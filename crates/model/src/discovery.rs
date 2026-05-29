//! Discovery surface — `## Lead inventory` blocks in
//! `discovery.md` and the lead shape source adapters emit at
//! `survey` time. Validated against
//! `schemas/discovery/lead.schema.json`.
//!
//! The whole-document model lives in [`document`] (discovery alias contract); it
//! parses `discovery.md`, exposes [`Discovery::resolve_lead`]
//! for the `--sources <key>=<id-or-alias>` rewrite path, and gates
//! the cross-lead `id` ↔ `aliases[]` collision check shared
//! between `specrun plan amend --add-alias` and
//! `specrun slice validate`.

pub mod document;
pub mod lead;

pub use document::{Discovery, DiscoveryAliasCollision, ResolveError as DiscoveryResolveError};
pub use lead::{AliasCollision, Lead, LeadAliases};
