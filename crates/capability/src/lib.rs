//! Capability resolution + brief frontmatter parsing.
//!
//! This crate is the canonical home for:
//!
//! - Parsing capability manifests (still on disk as `schema.yaml`
//!   pre-chunk-1.4; renamed to `capability.yaml` afterwards) — see
//!   [`Capability`].
//! - Resolving a `capability` value from `.specify/project.yaml` to
//!   either a workspace directory or the agent-populated cache
//!   (see [`Capability::resolve`]). Remote (HTTP) resolution is
//!   explicitly the agent's job per RFC-1 — the CLI only walks the
//!   filesystem.
//! - Parsing YAML frontmatter on brief markdown files
//!   (see [`Brief`], [`BriefFrontmatter`]).
//! - The fully-resolved `capability + briefs` view used by almost every
//!   subcommand (see [`PipelineView`]).
//! - The on-disk `.cache-meta.yaml` format written by the agent
//!   (see [`CacheMeta`]).
//! - The on-disk `registry.yaml` platform catalogue at the repo root
//!   (see [`Registry`]) introduced by RFC-3a.
//! - The on-disk `initiative.md` operator-authored brief at the repo
//!   root (see [`InitiativeBrief`]) introduced by RFC-3a.

mod brief;
mod cache;
mod capability;
mod initiative_brief;
mod pipeline;
mod registry;

pub use brief::{Brief, BriefFrontmatter};
pub use cache::CacheMeta;
pub use capability::{Capability, CapabilitySource, Phase, Pipeline, PipelineEntry, ResolvedCapability};
pub use initiative_brief::{InitiativeBrief, InitiativeFrontmatter, InitiativeInput, InputKind};
pub use pipeline::PipelineView;
pub use registry::{ContractRoles, Registry, RegistryProject};

/// Outcome of a structural validation rule.
///
/// Canonical home for `ValidationResult`. `specify-validate` re-exports
/// this type so consumers can depend on either crate. See
/// `DECISIONS.md` (§"Change G — `ValidationResult` canonical home") for
/// the rationale — moving the type into `specify-validate` would close a
/// dependency cycle because `specify-validate` already depends on
/// `specify-capability` for `PipelineView`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationResult {
    /// Rule passed.
    Pass {
        /// Machine-readable rule identifier.
        rule_id: &'static str,
        /// Human-readable rule description.
        rule: &'static str,
    },
    /// Rule failed.
    Fail {
        /// Machine-readable rule identifier.
        rule_id: &'static str,
        /// Human-readable rule description.
        rule: &'static str,
        /// Detail message explaining the failure.
        detail: String,
    },
    /// Rule evaluation was deferred.
    Deferred {
        /// Machine-readable rule identifier.
        rule_id: &'static str,
        /// Human-readable rule description.
        rule: &'static str,
        /// Why the rule was deferred.
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests;
