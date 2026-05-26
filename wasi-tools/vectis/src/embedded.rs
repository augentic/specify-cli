//! Embedded schema sources shared between `validate` (lazy-compiled
//! validators) and `schema` (raw schema retrieval).

/// Canonical tool-owned `tokens.schema.json` (RFC-31 D1).
pub(crate) const TOKENS_SCHEMA_SOURCE: &str = include_str!("../embedded/tokens.schema.json");

/// Canonical tool-owned `assets.schema.json` (RFC-31 D1).
pub(crate) const ASSETS_SCHEMA_SOURCE: &str = include_str!("../embedded/assets.schema.json");

/// Canonical tool-owned `composition.schema.json` (RFC-31 D1).
/// Shared between `layout` mode (unwired-subset runtime) and
/// `composition` mode (full lifecycle runtime).
pub(crate) const COMPOSITION_SCHEMA_SOURCE: &str =
    include_str!("../embedded/composition.schema.json");
