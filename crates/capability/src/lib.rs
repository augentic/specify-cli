//! Capability resolution + brief frontmatter parsing.
//!
//! This crate is the canonical home for:
//!
//! - Parsing capability manifests (`capability.yaml`; the resolver
//!   still tolerates the pre-RFC-13 `schema.yaml` filename so internal
//!   callers like `init` keep working during the cut-over) — see
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
//! - The on-disk `change.md` operator-authored brief at the repo
//!   root (see [`ChangeBrief`]) introduced by RFC-3a as `initiative.md`
//!   and migrated to `change.md` by RFC-13 chunk 3.7.
//!
//! Registry parsing and shape validation moved out into
//! `specify-registry` in RFC-13 chunk 2.1; per the RFC's
//! "platform components are not capabilities" invariant this crate
//! must not depend on `specify-registry`.

mod brief;
mod cache;
mod capability;
mod change_brief;
mod pipeline;

pub use brief::{Brief, BriefFrontmatter};
pub use cache::CacheMeta;
pub use capability::{
    CAPABILITY_FILENAME, Capability, CapabilitySource, LEGACY_SCHEMA_FILENAME, ManifestProbe,
    Phase, Pipeline, PipelineEntry, ResolvedCapability,
};
pub use change_brief::{
    CHANGE_BRIEF_FILENAME, ChangeBrief, ChangeFrontmatter, ChangeInput, InputKind,
    LEGACY_CHANGE_BRIEF_FILENAME,
};
pub use pipeline::PipelineView;

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
