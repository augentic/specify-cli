//! Codex rule catalog.
//!
//! Owns per-rule frontmatter parsing and project-aware resolution that
//! composes parsed rules from the foundational `default` adapter, the
//! project's target adapter, future shared catalog sources, and the
//! repo-root `codex/` overlay.

pub mod resolver;
mod rule;

pub use resolver::{
    CODEX_DIR_NAME, CodexCatalogSource, CodexProvenance, CodexResolver, DEFAULT_CODEX_ADAPTER,
    ResolvedCodex, ResolvedCodexRule, adapter_name_from_value,
};
pub use rule::{CodexRule, CodexRuleFrontmatter, CodexSeverity};
