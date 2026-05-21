//! Adapter resolution and brief frontmatter parsing — the canonical
//! home for `Adapter`, `Brief`, `CacheMeta`, the codex catalogue, and
//! the resolved `PipelineView`.

#[expect(
    clippy::module_inception,
    reason = "preserves the per-concern split inherited from the pre-collapse `specify-adapter` crate; rename would cascade across many imports"
)]
mod adapter;
mod brief;
mod cache;
mod codex;
mod codex_resolver;
mod pipeline;

use std::borrow::Cow;

// --- Adapter core ---
pub use adapter::{
    ADAPTER_FILENAME, Adapter, AdapterSource, Phase, Pipeline, PipelineEntry, ResolvedAdapter,
    validate_against_schema,
};
// --- Brief frontmatter ---
pub use brief::{Brief, BriefFrontmatter};
// --- Agent-populated cache ---
pub use cache::CacheMeta;
// --- Codex (rules catalog) ---
pub use codex::{CodexRule, CodexRuleFrontmatter, CodexSeverity};
pub use codex_resolver::{
    CODEX_DIR_NAME, CodexCatalogSource, CodexProvenance, CodexResolver, DEFAULT_CODEX_ADAPTER,
    ResolvedCodex, ResolvedCodexRule,
};
pub use pipeline::PipelineView;

/// Outcome of a structural validation rule.
///
/// Canonical home for `ValidationResult`. Lives in `specify-domain`
/// because every consumer of the validation registry already depends
/// on the broader domain surface (`Adapter`, `PipelineView`,
/// `Registry`). See `DECISIONS.md` §"Crate layout" for the full
/// rationale.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
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
