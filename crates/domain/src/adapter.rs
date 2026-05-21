//! Adapter resolution and brief frontmatter parsing — the canonical
//! home for `Adapter`, `Brief`, `CacheMeta`, the codex catalogue, and
//! the resolved `PipelineView`.
//!
//! Validation outputs speak [`specify_error::ValidationSummary`] across
//! the domain crate; schema checks route through
//! [`crate::schema::validate_value`].

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

// --- Adapter core ---
pub use adapter::{
    ADAPTER_FILENAME, Adapter, AdapterSource, Phase, Pipeline, PipelineEntry, ResolvedAdapter,
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
