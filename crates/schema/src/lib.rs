//! Schema resolution + brief frontmatter parsing.
//!
//! This crate is the canonical home for:
//!
//! - Parsing `schema.yaml` (see [`Schema`]).
//! - Resolving a `schema` value from `.specify/project.yaml` to either a
//!   workspace directory or the agent-populated cache
//!   (see [`Schema::resolve`]). Remote (HTTP) resolution is explicitly the
//!   agent's job per RFC-1 — the CLI only walks the filesystem.
//! - Parsing YAML frontmatter on brief markdown files
//!   (see [`Brief`], [`BriefFrontmatter`]).
//! - The fully-resolved `schema + briefs` view used by almost every
//!   subcommand (see [`PipelineView`]).
//! - The on-disk `.cache-meta.yaml` format written by the agent
//!   (see [`CacheMeta`]).
//! - The on-disk `.specify/registry.yaml` platform catalogue
//!   (see [`Registry`]) introduced by RFC-3a.
//! - The on-disk `.specify/initiative.md` operator-authored brief
//!   (see [`InitiativeBrief`]) introduced by RFC-3a.

mod brief;
mod cache;
mod initiative_brief;
mod pipeline;
mod registry;
mod schema;

pub use brief::{Brief, BriefFrontmatter};
pub use cache::CacheMeta;
pub use initiative_brief::{InitiativeBrief, InitiativeFrontmatter, InitiativeInput, InputKind};
pub use pipeline::PipelineView;
pub use registry::{ContractRoles, Registry, RegistryProject};
pub use schema::{Phase, Pipeline, PipelineEntry, ResolvedSchema, Schema, SchemaSource};

/// Outcome of a structural validation rule.
///
/// Canonical home for `ValidationResult`. `specify-validate` re-exports
/// this type so consumers can depend on either crate. See
/// `DECISIONS.md` (§"Change G — `ValidationResult` canonical home") for
/// the rationale — moving the type into `specify-validate` would close a
/// dependency cycle because `specify-validate` already depends on
/// `specify-schema` for `PipelineView`.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    Pass { rule_id: &'static str, rule: &'static str },
    Fail { rule_id: &'static str, rule: &'static str, detail: String },
    Deferred { rule_id: &'static str, rule: &'static str, reason: &'static str },
}

#[cfg(test)]
mod tests;
