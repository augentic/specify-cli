//! Discovery surface — the `## Lead inventory` blocks in `discovery.md`.
//!
//! Each block is a raw, unmerged per-source lead a source adapter emits
//! at `survey` time, identified by its `(source-key, lead-id)` pair and
//! validated against `schemas/discovery/lead.schema.json`.
//!
//! The whole-document model lives in [`document`] (discovery alias contract); it
//! parses `discovery.md`, exposes [`Discovery::resolve_lead`]
//! for the `--sources <key>=<lead-id-or-alias>` rewrite path, and gates
//! the per-`source-key` `lead-id` ↔ `aliases[]` collision check shared
//! between `specrun plan amend --add-alias` and
//! `specrun slice validate`.

pub mod document;
pub mod lead;

pub use document::{Discovery, DiscoveryAliasCollision, ResolveError as DiscoveryResolveError};
pub use lead::{AliasCollision, Lead, LeadAliases};
