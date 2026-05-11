//! Capability resolution + brief frontmatter parsing.
//!
//! This crate is the canonical home for:
//!
//! - Parsing capability manifests (`capability.yaml`) — see
//!   [`Capability`].
//! - Resolving a `capability` value from `.specify/project.yaml` to
//!   either a workspace directory or the agent-populated cache
//!   (see [`Capability::resolve`]). Remote (HTTP) resolution is
//!   explicitly the agent's job — the CLI only walks the filesystem.
//! - Parsing YAML frontmatter on brief markdown files
//!   (see [`Brief`], [`BriefFrontmatter`]).
//! - The fully-resolved `capability + briefs` view used by almost every
//!   subcommand (see [`PipelineView`]).
//! - The on-disk `.cache-meta.yaml` format written by the agent
//!   (see [`CacheMeta`]).
//! - The on-disk `change.md` operator-authored brief at the repo
//!   root (see [`ChangeBrief`]).
//!
//! Registry parsing and shape validation live in `specify-registry`;
//! per the "platform components are not capabilities" invariant this
//! crate must not depend on `specify-registry`.

mod brief;
mod cache;
#[allow(
    clippy::module_inception,
    reason = "preserves the per-concern split inherited from the pre-collapse `specify-capability` crate; rename would cascade across many imports"
)]
mod capability;
mod change_brief;
mod codex;
mod codex_resolver;
mod pipeline;

use std::borrow::Cow;

// --- Brief frontmatter ---
pub use brief::{Brief, BriefFrontmatter};
// --- Agent-populated cache ---
pub use cache::CacheMeta;
// --- Capability core ---
pub use capability::{
    CAPABILITY_FILENAME, Capability, CapabilitySource, Phase, Pipeline, PipelineEntry,
    ResolvedCapability,
};
pub use change_brief::{
    ChangeBrief, ChangeFrontmatter, ChangeInput, FILENAME as CHANGE_BRIEF_FILENAME, InputKind,
};
// --- Codex (rules catalog) ---
pub use codex::{CodexRule, CodexRuleFrontmatter, CodexSeverity};
pub use codex_resolver::{
    CODEX_DIR_NAME, CodexCatalogSource, CodexProvenance, CodexResolver, DEFAULT_CODEX_CAPABILITY,
    ResolvedCodex, ResolvedCodexRule,
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
        rule_id: Cow<'static, str>,
        /// Human-readable rule description.
        rule: Cow<'static, str>,
    },
    /// Rule failed.
    Fail {
        /// Machine-readable rule identifier.
        rule_id: Cow<'static, str>,
        /// Human-readable rule description.
        rule: Cow<'static, str>,
        /// Detail message explaining the failure.
        detail: String,
    },
    /// Rule evaluation was deferred.
    Deferred {
        /// Machine-readable rule identifier.
        rule_id: Cow<'static, str>,
        /// Human-readable rule description.
        rule: Cow<'static, str>,
        /// Why the rule was deferred.
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests;
